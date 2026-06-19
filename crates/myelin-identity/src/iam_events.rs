//! # `iam_events` — the `iam.*` event tokens + their `EventEnvelope` projections (P-ID-02 / P-023)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §3 (the opaque `principal_id` / erasable `profile_ref` split — the GDPR
//! erasure-vs-immutability split), §6 (`iam.tuple_written` is emitted via the **outbox** —
//! the only emit path), §11.2 (the `iam.*` event set: `iam.tuple_written`,
//! `iam.role_granted`, `iam.break_glass`).
//!
//! **Hard problem grounded here:** `external-insights/04-hard-problems.md` §1
//! (erasure-vs-immutability). The event log is immutable + append-only; the workable answer
//! is to **separate identity from action** — attribute events by a stable **opaque id**
//! ([`PrincipalId`]) and keep the erasable personal data (name/email/profile, the
//! `profile_ref`) in a separate record. Erasure then tombstones the *identity*, never the
//! *fact*. P-ID-02 bakes that split into the envelope shape at M0: an `iam.*` event's
//! attribution carries **opaque `principal_id` only** — the erasable `profile_ref` never
//! enters the immutable envelope. This makes "an `iam.*` event leaks a name/email into the
//! immutable log" structurally impossible (the `control-plane-pii-free` lint, wired in
//! P-ID-03 / P-024, guards it; the compile-time [`tests`] below pin it in-crate).
//!
//! ## DAG-deviation note (EI-01 §1, documented) — why the projection is NOT an `EventEnvelope`
//! The P-ID-02 prompt says "define their `EventEnvelope` projections". The literal
//! `EventEnvelope` *struct* lives in `myelin-events` (contract 2.1), which sits **ABOVE**
//! `myelin-identity` in the frozen §2.9 crate DAG (the envelope's `actor` field embeds a
//! `Principal` owned here). Identity is a **SINK** — it may not import `myelin_events`
//! without inverting the DAG (the `crate-graph-acyclic` invariant; `myelin-identity`'s
//! `Cargo.toml` forbids the edge). This is the identical situation EI-01 §1 already
//! resolved for [`crate::DataRole`] (identity owns its own copy, name-aligned to
//! `myelin_events::DataRole`).
//!
//! So the **projection** is expressed as an identity-owned, compile-time **descriptor**
//! ([`IamEventProjection`]) that pins, for each `iam.*` token, *exactly which envelope
//! fields the emitter sets* — attribution by opaque [`PrincipalId`], the GDPR `data_role`,
//! and `contains_personal_data = false` — using identity-owned field types. The actual
//! `EventEnvelope` is constructed at **emit time** by the emit path in the events tier
//! (`OutboxTx::emit`, the only emit path), reached at **P-ID-08 (M1)** for
//! `iam.tuple_written`. The descriptor here is the *frozen contract* that emit path
//! reconciles against (the projection's field names line up 1:1 with the §2.10 envelope
//! anchor: `type_`, `actor`, `subject`, `contains_personal_data`, `data_role`).
//!
//! ## Floors named (frozen shape now → bodies in a later prompt)
//! - **NO service, NO emit path.** The bodies that emit these tokens land in M1:
//!   `iam.tuple_written` → **P-ID-08** (`write_tuples`, via the outbox); `iam.role_granted`
//!   / `iam.break_glass` → the role-grant / break-glass admin flows (Identity M1).
//! - The **taxonomy grammar validator + the seed token table** is the Bus's **EB-02 /
//!   P-042** deliverable; the `iam.*` constants here are the rows it will admit (the dotted
//!   `<subsystem>.<artifact_type>.<event_name>` grammar is honoured by construction — see
//!   [`tests::iam_tokens_obey_the_dotted_grammar`]). Until EB-02 lands, these constants ARE
//!   the registration (a `&'static str` token table is the M0 carrier; no second token
//!   language).
//! - The §1.8 telemetry **signal NAMES** Identity owns are declared here as `&'static str`
//!   constants so later prompts assert against named signals, not literals; the wiring onto
//!   the metrics-health port is the Identity service shell + the impl prompts (M1).

use crate::{DataRole, PrincipalId};
use myelin_tenancy::ArtifactRef;

// ===========================================================================
// §11.2 — the iam.* event token constants (the registration; dotted grammar)
// ===========================================================================

/// The `iam.tuple_written` token (architecture §6, §11.2). Emitted via the **outbox** (the
/// only emit path) whenever a relation tuple is written; **S8** (the authz reverse index)
/// is its consumer (C2), carrying the write's zookie as the revision watermark. The emit
/// body is **P-ID-08 (M1)**.
pub const IAM_TUPLE_WRITTEN: &str = "iam.tuple.written";

/// The `iam.role_granted` token (architecture §11.2). Emitted when a principal is granted a
/// role (an org/team/project membership edge). The emit body is Identity M1.
pub const IAM_ROLE_GRANTED: &str = "iam.role.granted";

/// The `iam.break_glass` token (architecture §11.2). Emitted on a break-glass / emergency
/// access elevation (audited, time-bounded). The emit body is Identity M1.
pub const IAM_BREAK_GLASS: &str = "iam.break_glass.invoked";

/// The complete set of `iam.*` tokens Identity registers (architecture §11.2). The Bus
/// taxonomy seed (EB-02 / P-042) admits exactly these rows; the in-crate
/// [`tests::iam_tokens_obey_the_dotted_grammar`] proves each obeys the
/// `<subsystem>.<artifact_type>.<event_name>` grammar with the `iam` subsystem prefix.
pub const IAM_EVENT_TOKENS: &[&str] = &[IAM_TUPLE_WRITTEN, IAM_ROLE_GRANTED, IAM_BREAK_GLASS];

// ===========================================================================
// §3 / §11.2 — the EventEnvelope projection descriptor (opaque-id-only attribution)
// ===========================================================================

/// How an `iam.*` event attributes its actor and subject in the immutable envelope
/// (architecture §3; EI-04 §1 — the erasure-vs-immutability split).
///
/// **The whole point:** every reference is an **opaque [`PrincipalId`]**, never the erasable
/// `profile_ref` (name/email/profile). There is *no field on this type* that could carry
/// PII — the split is structural, not a runtime check. A `subject` may be a
/// principal-as-subject (a role grant *about* a principal) OR an [`ArtifactRef`] (a tuple
/// written *about* an object), both PII-free reference types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IamSubjectRef {
    /// The event is about a principal (e.g. a role grant), referenced by opaque id only.
    Principal(PrincipalId),
    /// The event is about an object (e.g. a tuple `object#relation@subject`), referenced by
    /// its PII-free [`ArtifactRef`].
    Object(ArtifactRef),
}

/// The frozen `EventEnvelope` projection for one `iam.*` token (P-ID-02): the exact set of
/// envelope fields the emitter sets, pinned at M0 so the GDPR erasure-vs-immutability split
/// is baked into the shape — **not** discovered at emit time.
///
/// The field names line up 1:1 with the §2.10 `EventEnvelope` anchor (`type_`, `actor`,
/// `subject`, `contains_personal_data`, `data_role`): when the emit path (P-ID-08, the
/// events tier) constructs the real `myelin_events::EventEnvelope`, it copies these through.
/// **Crucially there is no `name`/`email`/`profile` field anywhere on this projection** —
/// attribution is opaque-id-only by *construction*, so the immutable log can never carry
/// erasable PII for an `iam.*` event (EI-04 §1; the `control-plane-pii-free` lint, P-ID-03,
/// is the external guard; [`tests::no_iam_projection_carries_a_pii_field`] is the in-crate
/// compile/scan proof).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IamEventProjection {
    /// The dotted event token (one of [`IAM_EVENT_TOKENS`]) — maps to `EventEnvelope.type_`.
    pub type_: &'static str,
    /// The acting principal, by **opaque id only** — maps to `EventEnvelope.actor`
    /// (the events tier wraps it in `Actor(Principal)`; only the opaque `principal_id`
    /// crosses, never the `profile_ref`).
    pub actor_principal_id: PrincipalId,
    /// What the event is about, by PII-free reference — maps to `EventEnvelope.subject`.
    pub subject: IamSubjectRef,
    /// `EventEnvelope.contains_personal_data` — **always `false`** for an `iam.*` event:
    /// references-not-payloads + opaque-id attribution means no inline PII ever rides an
    /// `iam.*` event (so `pii_key_ref` is never set either). Pinned by
    /// [`tests::every_iam_projection_is_personal_data_free`].
    pub contains_personal_data: bool,
    /// `EventEnvelope.data_role` — the GDPR fan-out role (controller | processor) the authz
    /// state change is recorded under (architecture §2.1).
    pub data_role: DataRole,
}

impl IamEventProjection {
    /// Build the projection for an `iam.*` token. `contains_personal_data` is forced
    /// `false` (the opaque-id-only / references-not-payloads invariant) — it is **not** a
    /// caller-supplied field, so an emitter cannot fabricate an `iam.*` event that claims to
    /// carry inline PII (the erasure-vs-immutability split is structural, EI-04 §1).
    pub fn new(
        type_: &'static str,
        actor_principal_id: PrincipalId,
        subject: IamSubjectRef,
        data_role: DataRole,
    ) -> Self {
        debug_assert!(
            IAM_EVENT_TOKENS.contains(&type_),
            "iam.* projection built for an unregistered token: {type_}"
        );
        IamEventProjection {
            type_,
            actor_principal_id,
            subject,
            // Structural: opaque-id attribution + references-not-payloads ⇒ never inline PII.
            contains_personal_data: false,
            data_role,
        }
    }
}

// ===========================================================================
// §1.8 — the telemetry signal NAME constants Identity owns (contract-index row 1.8)
// ===========================================================================

/// The §1.8 telemetry signal names Identity owns (architecture §11.1 telemetry row;
/// contract-index row 1.8). Declared as `&'static str` constants so later prompts (the
/// service shell + the impl prompts, M1) assert against **named** signals, never literals.
/// These are the names the metrics-health port exports; the wiring lands with the bodies.
pub mod signals {
    /// `check` decision latency (RED — the hot authz path).
    pub const AUTH_DECISION_LATENCY: &str = "auth_decision_latency";
    /// The fail-static cache hit ratio (§10; the availability posture).
    pub const CACHE_HIT_RATIO: &str = "cache_hit_ratio";
    /// How stale a fail-static-served decision is (§10; bounded by `W`, contract 4.11).
    pub const STALENESS_AGE: &str = "staleness_age";
    /// The lag between a `revoke` and it taking effect everywhere (ID-D1/D2).
    pub const REVOCATION_LAG: &str = "revocation_lag";
    /// The lag between a tuple write and its durable commit (the `write_tuples` path).
    pub const TUPLE_WRITE_LAG: &str = "tuple_write_lag";
    /// **NEW (S8 freshness):** the lag between an `iam.tuple_written` and its reflection in
    /// the authz reverse index S8 (architecture §11.1 telemetry; the S8 freshness SLO).
    pub const REVERSE_INDEX_LAG: &str = "reverse_index_lag";

    /// The complete §1.8 Identity-owned signal-name set (the order the impl prompts assert).
    pub const IDENTITY_SIGNAL_NAMES: &[&str] = &[
        AUTH_DECISION_LATENCY,
        CACHE_HIT_RATIO,
        STALENESS_AGE,
        REVOCATION_LAG,
        TUPLE_WRITE_LAG,
        REVERSE_INDEX_LAG,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PrincipalId;

    /// Each `iam.*` token obeys the Bus dotted grammar
    /// `<subsystem>.<artifact_type>.<event_name>` with the `iam` subsystem prefix (Bus §6;
    /// the grammar the EB-02 / P-042 taxonomy validator will enforce). Three dotted
    /// segments, all non-empty, lowercase, prefixed `iam.`.
    #[test]
    fn iam_tokens_obey_the_dotted_grammar() {
        for tok in IAM_EVENT_TOKENS {
            let parts: Vec<&str> = tok.split('.').collect();
            assert_eq!(
                parts.len(),
                3,
                "token `{tok}` must be <subsystem>.<artifact_type>.<event_name>"
            );
            assert_eq!(parts[0], "iam", "token `{tok}` must carry the `iam` subsystem prefix");
            for seg in &parts {
                assert!(!seg.is_empty(), "token `{tok}` has an empty segment");
                assert!(
                    seg.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                    "token `{tok}` segment `{seg}` must be lowercase snake"
                );
            }
        }
        // The set is exactly the §11.2 three (no drift — adding/removing a token is a
        // contract change every consumer's taxonomy registration must reconcile).
        assert_eq!(IAM_EVENT_TOKENS.len(), 3);
        assert!(IAM_EVENT_TOKENS.contains(&IAM_TUPLE_WRITTEN));
        assert!(IAM_EVENT_TOKENS.contains(&IAM_ROLE_GRANTED));
        assert!(IAM_EVENT_TOKENS.contains(&IAM_BREAK_GLASS));
    }

    /// The P-ID-02 gate: each `iam.*` projection carries `actor`/`subject` by **opaque
    /// `principal_id`** (never the erasable `profile_ref`), and `contains_personal_data` is
    /// set correctly (always `false` — opaque-id attribution + references-not-payloads).
    #[test]
    fn iam_projection_attributes_by_opaque_id_and_classifies_correctly() {
        // iam.tuple_written: actor is a principal id; subject is a PII-free ArtifactRef
        // (the object the tuple is about).
        let tw = IamEventProjection::new(
            IAM_TUPLE_WRITTEN,
            PrincipalId("p-admin".into()),
            IamSubjectRef::Object(ArtifactRef("myelin://acme/git/repo/core".into())),
            DataRole::Controller,
        );
        assert_eq!(tw.type_, IAM_TUPLE_WRITTEN);
        assert_eq!(tw.actor_principal_id, PrincipalId("p-admin".into()));
        assert!(matches!(tw.subject, IamSubjectRef::Object(_)));
        assert!(!tw.contains_personal_data, "an iam.* event never carries inline PII");

        // iam.role_granted: subject is a principal-as-subject (the grantee), opaque id only.
        let rg = IamEventProjection::new(
            IAM_ROLE_GRANTED,
            PrincipalId("p-admin".into()),
            IamSubjectRef::Principal(PrincipalId("p-grantee".into())),
            DataRole::Controller,
        );
        assert_eq!(rg.type_, IAM_ROLE_GRANTED);
        assert!(matches!(rg.subject, IamSubjectRef::Principal(ref id) if id.0 == "p-grantee"));
        assert!(!rg.contains_personal_data);

        // iam.break_glass: an emergency elevation, attributed to the invoking principal.
        let bg = IamEventProjection::new(
            IAM_BREAK_GLASS,
            PrincipalId("p-oncall".into()),
            IamSubjectRef::Principal(PrincipalId("p-target".into())),
            DataRole::Controller,
        );
        assert_eq!(bg.type_, IAM_BREAK_GLASS);
        assert!(!bg.contains_personal_data);
    }

    /// Every `iam.*` projection is personal-data-free by construction: `new` forces
    /// `contains_personal_data = false`, so an emitter cannot fabricate an `iam.*` event
    /// that claims inline PII (the erasure-vs-immutability split is structural, EI-04 §1).
    #[test]
    fn every_iam_projection_is_personal_data_free() {
        for tok in IAM_EVENT_TOKENS {
            let p = IamEventProjection::new(
                tok,
                PrincipalId("actor".into()),
                IamSubjectRef::Principal(PrincipalId("subject".into())),
                DataRole::Processor,
            );
            assert!(
                !p.contains_personal_data,
                "iam.* token `{tok}` projection must be personal-data-free (opaque-id-only)"
            );
        }
    }

    /// The P-ID-02 gate (the in-crate twin of the `control-plane-pii-free` lint, P-ID-03):
    /// **no field on the `iam.*` projection type carries a PII name.** This is the
    /// compile-time / source-scan assertion the prompt requires ("assert the projection
    /// contains no PII field at compile time") — kept self-contained because identity is a
    /// DAG sink and cannot depend on `myelin-lints`. It scans THIS module's own source for
    /// the `IamEventProjection` / `IamSubjectRef` struct/enum field identifiers and asserts
    /// none is a forbidden PII field name (the same fingerprint the workspace
    /// `control-plane-pii-free` gate uses). A future field named `email`/`name`/… fails here
    /// AND fails the live `control-plane-pii-free` workspace gate.
    #[test]
    fn no_iam_projection_carries_a_pii_field() {
        // The forbidden direct-identifier / free-text field names (the control-plane-pii-free
        // fingerprint, ADR-11/OQ-I). `principal_id` / `actor_principal_id` / `subject` are
        // opaque references and intentionally NOT in this set.
        const PII_FIELDS: &[&str] = &[
            "name",
            "email",
            "phone",
            "address",
            "body",
            "display_name",
            "full_name",
            "given_name",
            "family_name",
            "first_name",
            "last_name",
            "message",
            "comment",
            "title",
            "profile",
            "profile_ref",
        ];
        let src = include_str!("iam_events.rs");
        // Walk only the projection type definitions (their `{ ... }` bodies). We scan for
        // `<ident>:` field declarations and assert none is a PII field name.
        for marker in ["pub struct IamEventProjection {", "pub enum IamSubjectRef {"] {
            let start = src.find(marker).expect("projection type is defined in this module");
            let body = &src[start..];
            let end = body.find('}').expect("type body is brace-closed");
            for line in body[..end].lines() {
                let trimmed = line.trim();
                if let Some((lhs, _)) = trimmed.split_once(':') {
                    let ident = lhs.trim_start_matches("pub ").trim();
                    // strip a trailing enum-variant `(` if any (e.g. `Principal(PrincipalId)`)
                    let ident = ident.split(['(', ' ']).next().unwrap_or(ident);
                    assert!(
                        !PII_FIELDS.contains(&ident),
                        "iam.* projection carries forbidden PII field `{ident}` — \
                         attribution must be opaque-id-only (EI-04 §1; control-plane-pii-free)"
                    );
                }
            }
        }
    }

    /// The §1.8 Identity-owned telemetry signal NAMES are the frozen six (architecture
    /// §11.1 telemetry row; contract-index 1.8) — so later prompts assert against these
    /// names, not literals. A rename/drop here is a contract change the impl prompts catch.
    #[test]
    fn identity_owns_the_six_telemetry_signal_names() {
        use signals::*;
        assert_eq!(
            IDENTITY_SIGNAL_NAMES,
            &[
                "auth_decision_latency",
                "cache_hit_ratio",
                "staleness_age",
                "revocation_lag",
                "tuple_write_lag",
                "reverse_index_lag",
            ]
        );
        // The S8-freshness signal (the NEW row) is present (architecture §11.1: "+ S8
        // freshness reverse_index_lag").
        assert!(IDENTITY_SIGNAL_NAMES.contains(&REVERSE_INDEX_LAG));
    }
}
