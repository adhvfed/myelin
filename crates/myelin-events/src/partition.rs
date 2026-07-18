//! # `partition` — the `(tenant, region)`-keyed stream subject + per-(tenant, subsystem)
//! stream provisioning (EB-12 / P-089)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §2.2 (the subject encodes routing + ordering: `partition key = aggregate`,
//! `evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>`), §7.1–§7.3
//! (cell-local, per-(tenant, subsystem) streams, the tenant as the blast-radius + fairness unit).
//! **Contract:** `contract-index.md` row 12.1 (the `(tenant, region)` partition key — **CONSUMED**;
//! injected by the harness; the value types `TenantId`/`Region`/`ResidencyTag` are owned by
//! `myelin-tenancy`, the §2.9 DAG root, and re-exported by this crate).
//!
//! ## What EB-12 adds (and what it does NOT duplicate)
//! The §2.1 envelope already carries `tenant`/`region` as the FIRST-CLASS partition + residency
//! key (`envelope.rs`, EB-01), and the per-tenant **in-flight cap** that makes the tenant the
//! fairness/blast-radius unit already lives in [`crate::consumer`] (EB-05 —
//! [`crate::consumer::PerTenantInflight`]). What was MISSING is the **structured subject** that
//! §2.2 specifies — the routing + ordering key the JetStream stream filters on and orders within:
//!
//! ```text
//! evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>
//!    └────── stream filter (per (tenant, subsystem)) ──────┘
//!                              └──── ordering partition (the aggregate) ────┘
//! ```
//!
//! Before EB-12 the live NATS transport ([`crate::nats`]) slotted the relay's opaque
//! [`crate::ArtifactRef`] under the stream root as a single sanitised token — correct for
//! at-least-once delivery + broker dedup, but it did NOT encode the `(tenant, subsystem)` routing
//! split or the per-aggregate ordering partition. EB-12 promotes the subject to the §2.2 grammar so
//! a stream is filtered per `(tenant, subsystem)` (the blast-radius unit) and ordered per aggregate
//! (the linearisation key). [`StreamSubject`] is the one round-trippable encoding of that key;
//! [`stream_name_for`] is the per-(tenant, subsystem) stream the subject lands in.
//!
//! ## The partition key is `(tenant, region)` (contract 12.1, consumed)
//! The subject token carries the `tenant`; the **`region`** is the cell's residency pin — a stream
//! is provisioned inside exactly one cell, so the region is the cell's, not a subject token (a
//! subject never crosses a region; the residency-pin is EB-13). [`PartitionKey`] is the consumed
//! `(tenant, region)` pair the streams are keyed under — the CDC consumer side of 12.1 (the Bus
//! calls the partition key). It composes the two `myelin-tenancy` value types; it does not redefine
//! them (one authority, the DAG root — EI-01 §7).

use crate::{AggregateKey, EventEnvelope, EventType};
use myelin_tenancy::{Region, TenantId};

/// The `evt.` root token every stream subject starts with (architecture §2.2). A constant so the
/// grammar has one authority and a typo cannot fork the routing namespace.
pub const SUBJECT_ROOT: &str = "evt";

/// A deliberately finite bound for one routing token. NATS permits large subjects, but accepting
/// unbounded tenant or aggregate identifiers lets one corrupt outbox row create an oversized
/// protocol line. Existing canonical identifiers are far below this limit.
pub const MAX_SUBJECT_TOKEN_BYTES: usize = 255;

/// The maximum wire-subject size produced by the event bus, including the `evt.` root.
pub const MAX_STREAM_SUBJECT_BYTES: usize = 1024;

/// The `(tenant, region)` first-class partition key the streams are keyed under (contract 12.1,
/// **CONSUMED**; injected by the harness; ADR-11). This is the Bus's consumer side of 12.1: it
/// composes the two `myelin-tenancy` value types ([`TenantId`] + [`Region`]) into the partition the
/// per-(tenant, subsystem) streams live under. It deliberately holds NO mechanism beyond the key —
/// the residency-pin enforcement (no cross-region read path) is EB-13.
///
/// `tenant` is the subject-token routing key (it appears in the subject); `region` is the cell's
/// residency pin (a stream is provisioned in exactly one cell, so the region is the cell's and a
/// subject never crosses it). Together they are the partition under which a stream is provisioned.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PartitionKey {
    /// The tenant — the first column / blast-radius + fairness unit (§7.1; the EB-05 per-tenant
    /// in-flight cap is keyed on exactly this). Appears as a subject token (the routing key).
    pub tenant: TenantId,
    /// The cell's residency region (the residency pin; EB-13 enforces no cross-region read path).
    /// NOT a subject token — a stream lives in one region by construction.
    pub region: Region,
}

impl PartitionKey {
    /// The partition key the harness injects for a `(tenant, region)` pair (contract 12.1).
    pub fn new(tenant: TenantId, region: Region) -> Self {
        Self { tenant, region }
    }

    /// The partition key an [`EventEnvelope`] belongs to — read from the FIRST-CLASS `tenant` +
    /// `region` fields the §2.1 envelope already carries (never re-derived, never optional). This
    /// is the call site that makes the partition "true by construction": every event already names
    /// its partition, EB-12 only reads it.
    pub fn of(envelope: &EventEnvelope) -> Self {
        Self {
            tenant: envelope.tenant.clone(),
            region: envelope.region.clone(),
        }
    }
}

/// Why a subject (or the envelope it is built from) is malformed under the §2.2 grammar. Each
/// variant is a distinct, LOUD reason — the builder/parser never silently coerces (EI-01 §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubjectError {
    /// The `type_` (`<subsystem>.<artifact_type>.<event_name>`) had too few dotted segments to
    /// yield a subsystem + event_name (the §6.1 grammar needs ≥2).
    TypeTooShort { type_: String },
    /// The [`AggregateKey`] was not the `<aggregate_type>:<aggregate_id>` shape §2.2 expects (no
    /// `:` separator, or an empty half).
    MalformedAggregate { aggregate: String },
    /// A token would contain a `.` (the NATS subject delimiter) or be empty — it cannot be a single
    /// subject token. Carries which field produced the bad token.
    BadSubjectToken { field: &'static str, token: String },
    /// A token or the complete subject exceeded the event bus's finite protocol bound.
    SubjectTooLong {
        field: &'static str,
        bytes: usize,
        max_bytes: usize,
    },
    /// A parsed subject did not start with the [`SUBJECT_ROOT`] (`evt`) token.
    NotAnEventSubject { subject: String },
    /// A parsed subject did not have exactly the six `evt.<tenant>.<subsystem>.<aggregate_type>.
    /// <aggregate_id>.<event_name>` tokens.
    WrongTokenCount { subject: String, tokens: usize },
}

impl std::fmt::Display for SubjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubjectError::TypeTooShort { type_ } => write!(
                f,
                "type `{type_}` has too few segments — need <subsystem>.…​.<event_name> (≥2)"
            ),
            SubjectError::MalformedAggregate { aggregate } => write!(
                f,
                "aggregate `{aggregate}` is not <aggregate_type>:<aggregate_id>"
            ),
            SubjectError::BadSubjectToken { field, token } => write!(
                f,
                "{field} token `{token}` is empty or contains a subject delimiter, wildcard, or whitespace"
            ),
            SubjectError::SubjectTooLong {
                field,
                bytes,
                max_bytes,
            } => write!(f, "{field} is {bytes} bytes (maximum {max_bytes})"),
            SubjectError::NotAnEventSubject { subject } => {
                write!(
                    f,
                    "`{subject}` is not an `{SUBJECT_ROOT}.`-rooted event subject"
                )
            }
            SubjectError::WrongTokenCount { subject, tokens } => write!(
                f,
                "`{subject}` has {tokens} tokens — the grammar is exactly 6 \
                 (evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>)"
            ),
        }
    }
}

impl std::error::Error for SubjectError {}

/// A single NATS literal token must not contain separators, wildcards, whitespace, or control
/// characters. `:` and `/` remain valid because existing canonical aggregate identifiers use them.
fn validate_subject_token(field: &'static str, token: &str) -> Result<(), SubjectError> {
    if token.is_empty()
        || token
            .chars()
            .any(|c| c == '.' || c == '*' || c == '>' || c.is_whitespace() || c.is_control())
    {
        return Err(SubjectError::BadSubjectToken {
            field,
            token: token.to_string(),
        });
    }
    if token.len() > MAX_SUBJECT_TOKEN_BYTES {
        return Err(SubjectError::SubjectTooLong {
            field,
            bytes: token.len(),
            max_bytes: MAX_SUBJECT_TOKEN_BYTES,
        });
    }
    Ok(())
}

/// The structured stream subject — the §2.2 routing + ordering key, round-trippable to/from the
/// `evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>` wire form.
///
/// The first three tokens (`tenant`, `subsystem`) plus the `evt` root are the **stream filter**
/// (per (tenant, subsystem), cell-local — [`Self::stream_filter`]); the `(aggregate_type,
/// aggregate_id)` pair is the **ordering partition** (the aggregate — all of one aggregate's events
/// are totally ordered, §2.2/§2.3). The `event_name` is the leaf verb. This is the ONE encoding of
/// the partition + ordering key; emitters/transport build it via [`StreamSubject::of`] from the
/// envelope so the routing is true by construction, never hand-spelt.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StreamSubject {
    /// The tenant — the routing + blast-radius key (§7.1).
    pub tenant: TenantId,
    /// The producing subsystem (the `type_`'s leading token, §6.2).
    pub subsystem: String,
    /// The aggregate's type (the `AggregateKey`'s left half) — part of the ordering partition.
    pub aggregate_type: String,
    /// The aggregate's id (the `AggregateKey`'s right half) — the per-aggregate ordering key.
    pub aggregate_id: String,
    /// The event verb (the `type_`'s trailing token).
    pub event_name: String,
}

impl StreamSubject {
    /// Build the subject from an [`EventEnvelope`] — the call site §2.2 names. The subsystem and
    /// event_name are derived from the dotted `type_` (`<subsystem>.<artifact_type>.<event_name>`),
    /// the `aggregate_type` + `aggregate_id` from the `<type>:<id>` [`AggregateKey`], and the
    /// `tenant` is the FIRST-CLASS envelope field (never re-derived). Returns a LOUD
    /// [`SubjectError`] if any piece would not be a single subject token (so a malformed event never
    /// silently produces a wrong routing key — EI-01 §5).
    pub fn of(envelope: &EventEnvelope) -> Result<Self, SubjectError> {
        let (subsystem, event_name) = split_type(&envelope.type_)?;
        let (aggregate_type, aggregate_id) = split_aggregate(&envelope.aggregate)?;
        let tenant = envelope.tenant.clone();

        for (field, token) in [
            ("tenant", tenant.as_str()),
            ("subsystem", subsystem.as_str()),
            ("aggregate_type", aggregate_type.as_str()),
            ("aggregate_id", aggregate_id.as_str()),
            ("event_name", event_name.as_str()),
        ] {
            validate_subject_token(field, token)?;
        }

        let subject = Self {
            tenant,
            subsystem,
            aggregate_type,
            aggregate_id,
            event_name,
        };
        let bytes = subject.to_subject().len();
        if bytes > MAX_STREAM_SUBJECT_BYTES {
            return Err(SubjectError::SubjectTooLong {
                field: "subject",
                bytes,
                max_bytes: MAX_STREAM_SUBJECT_BYTES,
            });
        }
        Ok(subject)
    }

    /// The wire form: `evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>`
    /// (§2.2). This is what the JetStream publish targets; [`StreamSubject::parse`] is its exact
    /// inverse (the round-trip the EB-12 gate proves).
    pub fn to_subject(&self) -> String {
        format!(
            "{SUBJECT_ROOT}.{}.{}.{}.{}.{}",
            self.tenant.as_str(),
            self.subsystem,
            self.aggregate_type,
            self.aggregate_id,
            self.event_name,
        )
    }

    /// Parse a wire subject back into the structured key — the exact inverse of
    /// [`StreamSubject::to_subject`]. The aggregate_id may itself contain a `:` (the git per-ref
    /// `<repo>:<ref>` case, §2.2) but never a `.`; the six tokens are split on `.` only.
    pub fn parse(subject: &str) -> Result<Self, SubjectError> {
        if subject.len() > MAX_STREAM_SUBJECT_BYTES {
            return Err(SubjectError::SubjectTooLong {
                field: "subject",
                bytes: subject.len(),
                max_bytes: MAX_STREAM_SUBJECT_BYTES,
            });
        }
        let tokens: Vec<&str> = subject.split('.').collect();
        if tokens.len() != 6 {
            return Err(SubjectError::WrongTokenCount {
                subject: subject.to_string(),
                tokens: tokens.len(),
            });
        }
        if tokens[0] != SUBJECT_ROOT {
            return Err(SubjectError::NotAnEventSubject {
                subject: subject.to_string(),
            });
        }
        for (field, token) in [
            ("tenant", tokens[1]),
            ("subsystem", tokens[2]),
            ("aggregate_type", tokens[3]),
            ("aggregate_id", tokens[4]),
            ("event_name", tokens[5]),
        ] {
            validate_subject_token(field, token)?;
        }
        Ok(Self {
            tenant: TenantId(tokens[1].to_string()),
            subsystem: tokens[2].to_string(),
            aggregate_type: tokens[3].to_string(),
            aggregate_id: tokens[4].to_string(),
            event_name: tokens[5].to_string(),
        })
    }

    /// The per-(tenant, subsystem) **stream filter** this subject lands in (§2.2/§7.1): every event
    /// for this `(tenant, subsystem)` matches `evt.<tenant>.<subsystem>.>`. This is the filter the
    /// cell-local stream is provisioned with — one stream per (tenant, subsystem), the tenant the
    /// blast-radius unit. A consumer binds a NON-`*` subject under this (the §5 consumer template's
    /// `*`-rejection is unaffected — `>` here is the *stream* filter, not a consumer subscription).
    pub fn stream_filter(&self) -> String {
        format!(
            "{SUBJECT_ROOT}.{}.{}.>",
            self.tenant.as_str(),
            self.subsystem
        )
    }

    /// The ordering partition (the aggregate) — `<aggregate_type>:<aggregate_id>` (§2.2/§2.3). All
    /// events sharing this are totally ordered (the per-aggregate linearisation key; global order is
    /// explicitly NOT promised). Equal to the originating [`AggregateKey`].
    pub fn ordering_partition(&self) -> AggregateKey {
        AggregateKey(format!("{}:{}", self.aggregate_type, self.aggregate_id))
    }
}

/// The cell-local **stream name** a `(tenant, subsystem)` pair is provisioned under (§7.1: "Streams
/// are per (tenant, subsystem), partitioned by `aggregate_id`"). One stream per (tenant, subsystem)
/// — the tenant is the blast-radius + fairness unit (the EB-05 per-tenant in-flight cap is now
/// tenant-real: one tenant's surge is isolated to its own stream). The region is the cell's, not a
/// name token (the stream lives in one region by construction — residency-pin is EB-13).
///
/// `EVT_<tenant>_<subsystem>` — the `EVT_` prefix keeps the stream namespace distinct, and the two
/// tokens are joined with `_` (JetStream stream names disallow `.`/`*`/`>`/whitespace; the subject
/// tokens are already subject-safe so they are name-safe too).
pub fn stream_name_for(partition: &PartitionKey, subsystem: &str) -> String {
    format!("EVT_{}_{}", partition.tenant.as_str(), subsystem)
}

/// Split a dotted `type_` (`<subsystem>.<artifact_type>.<event_name>`) into `(subsystem,
/// event_name)` — the two ends §2.2 lifts into the subject. The middle `artifact_type` is the
/// `type_`'s, distinct from the AGGREGATE's type (which comes from the `AggregateKey`); the subject
/// carries the aggregate's type, so only the subsystem + event_name are taken from `type_`.
fn split_type(type_: &EventType) -> Result<(String, String), SubjectError> {
    let segments: Vec<&str> = type_.0.split('.').collect();
    if segments.len() < 2 {
        return Err(SubjectError::TypeTooShort {
            type_: type_.0.clone(),
        });
    }
    let subsystem = segments[0].to_string();
    let event_name = segments[segments.len() - 1].to_string();
    Ok((subsystem, event_name))
}

/// Split an [`AggregateKey`] (`<aggregate_type>:<aggregate_id>`, §2.2) into its two halves. The id
/// half may itself contain `:` (the git per-ref `<repo>:<ref>` case) — we split on the FIRST `:`
/// only, so `ref.<repo>:<ref_name>` style ids survive intact. Both halves must be non-empty.
fn split_aggregate(aggregate: &AggregateKey) -> Result<(String, String), SubjectError> {
    match aggregate.0.split_once(':') {
        Some((ty, id)) if !ty.is_empty() && !id.is_empty() => Ok((ty.to_string(), id.to_string())),
        _ => Err(SubjectError::MalformedAggregate {
            aggregate: aggregate.0.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, ArtifactRef, CorrelationId, DataRole, EventId, PiiKeyRef, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    /// A local self-contained envelope fixture (the partition tests own their fixture rather than
    /// reaching into the private `envelope::tests` module). `type_ = issue.issue.created`,
    /// `aggregate = issue:PROJ-1`, tenant `acme`, region `fr-par` (the dev region).
    fn sample_envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType("issue.issue.created".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None::<PiiKeyRef>,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    /// **The EB-12 GATE (subject round-trip).** The subject grammar round-trips the
    /// `(tenant, subsystem, aggregate_type, aggregate_id, event_name)` routing + ordering key:
    /// `parse(to_subject(s)) == s` for a representative event, and the wire form is exactly the
    /// §2.2 grammar.
    #[test]
    fn subject_grammar_round_trips_the_routing_and_ordering_key() {
        let subject = StreamSubject {
            tenant: TenantId("acme".into()),
            subsystem: "issue".into(),
            aggregate_type: "issue".into(),
            aggregate_id: "PROJ-1".into(),
            event_name: "created".into(),
        };
        let wire = subject.to_subject();
        assert_eq!(wire, "evt.acme.issue.issue.PROJ-1.created");
        assert_eq!(StreamSubject::parse(&wire).unwrap(), subject);
        // The ordering partition is the aggregate (the §2.3 linearisation key).
        assert_eq!(
            subject.ordering_partition(),
            AggregateKey("issue:PROJ-1".into())
        );
        // The stream filter is per (tenant, subsystem) (the §7.1 blast-radius unit).
        assert_eq!(subject.stream_filter(), "evt.acme.issue.>");
    }

    /// The git per-ref case (§2.2): the aggregate id carries a `:` (`<repo>:<ref_name>`) and the
    /// subject still round-trips exactly — the `:` is NOT the subject delimiter, only `.` is.
    #[test]
    fn git_per_ref_aggregate_id_with_colon_round_trips() {
        let subject = StreamSubject {
            tenant: TenantId("acme".into()),
            subsystem: "git".into(),
            aggregate_type: "ref".into(),
            aggregate_id: "repo42:refs/heads/main".into(),
            event_name: "updated".into(),
        };
        let wire = subject.to_subject();
        // Exactly six dot-tokens — the `:`s and `/`s live INSIDE the aggregate_id token.
        assert_eq!(wire.split('.').count(), 6);
        let parsed = StreamSubject::parse(&wire).unwrap();
        assert_eq!(parsed, subject);
        assert_eq!(parsed.aggregate_id, "repo42:refs/heads/main");
    }

    /// [`StreamSubject::of`] derives the subject from a real envelope: subsystem + event_name from
    /// the dotted `type_`, `(aggregate_type, aggregate_id)` from the `<type>:<id>` aggregate, tenant
    /// from the first-class envelope field. The derived key round-trips.
    #[test]
    fn of_derives_the_subject_from_a_real_envelope() {
        let env = sample_envelope(); // type=issue.issue.created, aggregate=issue:PROJ-1
        let subject = StreamSubject::of(&env).unwrap();
        assert_eq!(subject.subsystem, "issue");
        assert_eq!(subject.event_name, "created");
        assert_eq!(subject.aggregate_type, "issue");
        assert_eq!(subject.aggregate_id, "PROJ-1");
        assert_eq!(subject.tenant, env.tenant);
        // Round-trips through the wire form.
        let wire = subject.to_subject();
        assert_eq!(StreamSubject::parse(&wire).unwrap(), subject);
    }

    /// The CDC consumer side of contract 12.1: the Bus CALLS the `(tenant, region)` partition key —
    /// it reads the first-class envelope fields, never re-derives them, and composes the two
    /// `myelin-tenancy` value types into [`PartitionKey`] (the consumed shape).
    #[test]
    fn partition_key_is_the_consumed_tenant_region_pair() {
        let env = sample_envelope();
        let pk = PartitionKey::of(&env);
        assert_eq!(pk.tenant, env.tenant);
        assert_eq!(pk.region, env.region);
        // Composed from the two value types directly (the DAG-root authority, no redefinition).
        let pk2 = PartitionKey::new(env.tenant.clone(), env.region.clone());
        assert_eq!(pk, pk2);
        // The stream a (tenant, subsystem) lands in is per-(tenant, subsystem), cell-local.
        let subject = StreamSubject::of(&env).unwrap();
        assert_eq!(stream_name_for(&pk, &subject.subsystem), "EVT_acme_issue");
    }

    /// Two tenants emitting the SAME subsystem/aggregate/event land in DISTINCT streams — the
    /// per-(tenant, subsystem) provisioning that makes the tenant the blast-radius unit (§7.1): one
    /// tenant's stream is structurally separate from another's (the bulkhead the EB-05 per-tenant
    /// in-flight cap then enforces in-flight).
    #[test]
    fn distinct_tenants_get_distinct_streams_the_bulkhead_property() {
        let region = Region("fr-par".into());
        let acme = PartitionKey::new(TenantId("acme".into()), region.clone());
        let globex = PartitionKey::new(TenantId("globex".into()), region);
        assert_ne!(
            stream_name_for(&acme, "issue"),
            stream_name_for(&globex, "issue"),
            "one tenant's stream must be structurally isolated from another's (§7.1)"
        );
        // And the subjects never collide across tenants (the tenant is the first routing token).
        let s_acme = StreamSubject {
            tenant: acme.tenant.clone(),
            subsystem: "issue".into(),
            aggregate_type: "issue".into(),
            aggregate_id: "X".into(),
            event_name: "created".into(),
        };
        let s_globex = StreamSubject {
            tenant: globex.tenant.clone(),
            ..s_acme.clone()
        };
        assert_ne!(s_acme.to_subject(), s_globex.to_subject());
        // A cross-tenant stream filter never captures another tenant's subject.
        assert!(!s_globex
            .to_subject()
            .starts_with(&s_acme.stream_filter().trim_end_matches('>').to_string()));
    }

    /// Malformed inputs are LOUD, never silently coerced (EI-01 §5).
    #[test]
    fn malformed_inputs_are_rejected_with_their_rule() {
        // A `type_` with one segment cannot yield a subsystem + event_name.
        let mut env = sample_envelope();
        env.type_ = EventType("created".into());
        assert!(matches!(
            StreamSubject::of(&env),
            Err(SubjectError::TypeTooShort { .. })
        ));
        // An aggregate without the `:` separator.
        let mut env2 = sample_envelope();
        env2.aggregate = AggregateKey("noseparator".into());
        assert!(matches!(
            StreamSubject::of(&env2),
            Err(SubjectError::MalformedAggregate { .. })
        ));
        // A subject with the wrong token count.
        assert!(matches!(
            StreamSubject::parse("evt.acme.issue.issue.PROJ-1"),
            Err(SubjectError::WrongTokenCount { tokens: 5, .. })
        ));
        // A subject not rooted at `evt`.
        assert!(matches!(
            StreamSubject::parse("dlq.acme.issue.issue.PROJ-1.created"),
            Err(SubjectError::NotAnEventSubject { .. })
        ));
        // A tenant token carrying a `.` would fork the routing namespace — rejected.
        let mut env3 = sample_envelope();
        env3.tenant = TenantId("ac.me".into());
        assert!(matches!(
            StreamSubject::of(&env3),
            Err(SubjectError::BadSubjectToken {
                field: "tenant",
                ..
            })
        ));

        for unsafe_tenant in ["ac me", "acme*", "acme>", "acme\n"] {
            let mut unsafe_env = sample_envelope();
            unsafe_env.tenant = TenantId(unsafe_tenant.into());
            assert!(matches!(
                StreamSubject::of(&unsafe_env),
                Err(SubjectError::BadSubjectToken {
                    field: "tenant",
                    ..
                })
            ));
        }

        let mut oversized = sample_envelope();
        oversized.aggregate = AggregateKey(format!("issue:{}", "x".repeat(256)));
        assert!(matches!(
            StreamSubject::of(&oversized),
            Err(SubjectError::SubjectTooLong {
                field: "aggregate_id",
                ..
            })
        ));

        assert!(matches!(
            StreamSubject::parse("evt.acme.issue.issue.*.created"),
            Err(SubjectError::BadSubjectToken {
                field: "aggregate_id",
                ..
            })
        ));
    }

    /// The subject is exactly the §2.2 grammar string (no drift): the documented wire shape.
    #[test]
    fn subject_is_exactly_the_section_2_2_grammar() {
        let env = sample_envelope();
        let wire = StreamSubject::of(&env).unwrap().to_subject();
        assert_eq!(wire, "evt.acme.issue.issue.PROJ-1.created");
    }
}
