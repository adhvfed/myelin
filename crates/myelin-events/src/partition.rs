use crate::{AggregateKey, EventEnvelope, EventType};
use myelin_tenancy::{Region, TenantId};

pub const SUBJECT_ROOT: &str = "evt";

pub const MAX_SUBJECT_TOKEN_BYTES: usize = 255;

pub const MAX_STREAM_SUBJECT_BYTES: usize = 1024;

pub const MAX_ENCODED_COMPONENT_BYTES: usize = MAX_STREAM_SUBJECT_BYTES;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SubjectComponent(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectComponentError {
    Empty,
    NonCanonical,
    TooLong,
}

impl std::fmt::Display for SubjectComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "subject component is empty"),
            Self::NonCanonical => write!(f, "subject component encoding is non-canonical"),
            Self::TooLong => write!(f, "subject component exceeds its bounded size"),
        }
    }
}

impl std::error::Error for SubjectComponentError {}

impl SubjectComponent {
    pub fn encode(raw: &str) -> Result<Self, SubjectComponentError> {
        if raw.is_empty() {
            return Err(SubjectComponentError::Empty);
        }
        let mut encoded = String::with_capacity(raw.len().min(MAX_ENCODED_COMPONENT_BYTES + 1));
        for byte in raw.as_bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                encoded.push(*byte as char);
            } else {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
            if encoded.len() > MAX_ENCODED_COMPONENT_BYTES {
                return Err(SubjectComponentError::TooLong);
            }
        }
        Ok(Self(encoded))
    }

    pub fn parse(encoded: &str) -> Result<Self, SubjectComponentError> {
        if encoded.is_empty() {
            return Err(SubjectComponentError::Empty);
        }
        if encoded.len() > MAX_ENCODED_COMPONENT_BYTES {
            return Err(SubjectComponentError::TooLong);
        }
        let bytes = encoded.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'%' => {
                    if index + 2 >= bytes.len() {
                        return Err(SubjectComponentError::NonCanonical);
                    }
                    let hi = decode_upper_hex(bytes[index + 1])
                        .ok_or(SubjectComponentError::NonCanonical)?;
                    let lo = decode_upper_hex(bytes[index + 2])
                        .ok_or(SubjectComponentError::NonCanonical)?;
                    decoded.push((hi << 4) | lo);
                    index += 3;
                }
                byte if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') => {
                    decoded.push(byte);
                    index += 1;
                }
                _ => return Err(SubjectComponentError::NonCanonical),
            }
        }
        let raw = std::str::from_utf8(&decoded).map_err(|_| SubjectComponentError::NonCanonical)?;
        let canonical = Self::encode(raw)?;
        if canonical.0 != encoded {
            return Err(SubjectComponentError::NonCanonical);
        }
        Ok(canonical)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn decode(&self) -> String {
        let bytes = self.0.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                let hi = decode_upper_hex(bytes[index + 1]).expect("validated uppercase escape");
                let lo = decode_upper_hex(bytes[index + 2]).expect("validated uppercase escape");
                decoded.push((hi << 4) | lo);
                index += 3;
            } else {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(decoded).expect("validated UTF-8 component")
    }
}

fn decode_upper_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PartitionKey {
    pub tenant: TenantId,
    pub region: Region,
}

impl PartitionKey {
    pub fn new(tenant: TenantId, region: Region) -> Self {
        Self { tenant, region }
    }

    pub fn of(envelope: &EventEnvelope) -> Self {
        Self {
            tenant: envelope.tenant.clone(),
            region: envelope.region.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubjectError {
    TypeTooShort { type_: String },
    MalformedAggregate { aggregate: String },
    BadSubjectToken { field: &'static str, token: String },
    SubjectTooLong {
        field: &'static str,
        bytes: usize,
        max_bytes: usize,
    },
    NotAnEventSubject { subject: String },
    WrongTokenCount { subject: String, tokens: usize },
}

impl std::fmt::Display for SubjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubjectError::TypeTooShort { type_ } => write!(
                f,
                "type `{type_}` has too few segments - need <subsystem>.…​.<event_name> (≥2)"
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
                "`{subject}` has {tokens} tokens - the grammar is exactly 6 \
                 (evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>)"
            ),
        }
    }
}

impl std::error::Error for SubjectError {}

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StreamSubject {
    pub tenant: TenantId,
    pub subsystem: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub event_name: String,
}

impl StreamSubject {
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

    pub fn stream_filter(&self) -> String {
        format!(
            "{SUBJECT_ROOT}.{}.{}.>",
            self.tenant.as_str(),
            self.subsystem
        )
    }

    pub fn ordering_partition(&self) -> AggregateKey {
        AggregateKey(format!("{}:{}", self.aggregate_type, self.aggregate_id))
    }
}

pub fn stream_name_for(partition: &PartitionKey, subsystem: &str) -> String {
    format!("EVT_{}_{}", partition.tenant.as_str(), subsystem)
}

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
        assert_eq!(
            subject.ordering_partition(),
            AggregateKey("issue:PROJ-1".into())
        );
        assert_eq!(subject.stream_filter(), "evt.acme.issue.>");
    }

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
        assert_eq!(wire.split('.').count(), 6);
        let parsed = StreamSubject::parse(&wire).unwrap();
        assert_eq!(parsed, subject);
        assert_eq!(parsed.aggregate_id, "repo42:refs/heads/main");
    }

    #[test]
    fn of_derives_the_subject_from_a_real_envelope() {
        let env = sample_envelope();
        let subject = StreamSubject::of(&env).unwrap();
        assert_eq!(subject.subsystem, "issue");
        assert_eq!(subject.event_name, "created");
        assert_eq!(subject.aggregate_type, "issue");
        assert_eq!(subject.aggregate_id, "PROJ-1");
        assert_eq!(subject.tenant, env.tenant);
        let wire = subject.to_subject();
        assert_eq!(StreamSubject::parse(&wire).unwrap(), subject);
    }

    #[test]
    fn partition_key_is_the_consumed_tenant_region_pair() {
        let env = sample_envelope();
        let pk = PartitionKey::of(&env);
        assert_eq!(pk.tenant, env.tenant);
        assert_eq!(pk.region, env.region);
        let pk2 = PartitionKey::new(env.tenant.clone(), env.region.clone());
        assert_eq!(pk, pk2);
        let subject = StreamSubject::of(&env).unwrap();
        assert_eq!(stream_name_for(&pk, &subject.subsystem), "EVT_acme_issue");
    }

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
        assert!(!s_globex
            .to_subject()
            .starts_with(&s_acme.stream_filter().trim_end_matches('>').to_string()));
    }

    #[test]
    fn malformed_inputs_are_rejected_with_their_rule() {
        let mut env = sample_envelope();
        env.type_ = EventType("created".into());
        assert!(matches!(
            StreamSubject::of(&env),
            Err(SubjectError::TypeTooShort { .. })
        ));
        let mut env2 = sample_envelope();
        env2.aggregate = AggregateKey("noseparator".into());
        assert!(matches!(
            StreamSubject::of(&env2),
            Err(SubjectError::MalformedAggregate { .. })
        ));
        assert!(matches!(
            StreamSubject::parse("evt.acme.issue.issue.PROJ-1"),
            Err(SubjectError::WrongTokenCount { tokens: 5, .. })
        ));
        assert!(matches!(
            StreamSubject::parse("dlq.acme.issue.issue.PROJ-1.created"),
            Err(SubjectError::NotAnEventSubject { .. })
        ));
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

    #[test]
    fn subject_is_exactly_the_section_2_2_grammar() {
        let env = sample_envelope();
        let wire = StreamSubject::of(&env).unwrap().to_subject();
        assert_eq!(wire, "evt.acme.issue.issue.PROJ-1.created");
    }

    #[test]
    fn subject_component_is_reversible_and_delimiter_safe() {
        let raw = "repo.with/slash:and%percent#anchor";
        let component = SubjectComponent::encode(raw).unwrap();
        assert_eq!(
            component.as_str(),
            "repo%2Ewith%2Fslash%3Aand%25percent%23anchor"
        );
        assert_eq!(component.decode(), raw);
        assert_eq!(
            SubjectComponent::parse(component.as_str()).unwrap(),
            component
        );
    }

    #[test]
    fn subject_component_rejects_non_canonical_encodings() {
        for encoded in ["repo%2ewith", "repo%2F%", "repo%41", "repo.with", ""] {
            assert!(
                SubjectComponent::parse(encoded).is_err(),
                "{encoded:?} must not acquire a second wire spelling"
            );
        }
    }

    #[test]
    fn subject_component_has_a_finite_encoded_bound() {
        let long_but_valid = "a".repeat(MAX_SUBJECT_TOKEN_BYTES + 1);
        let component = SubjectComponent::encode(&long_but_valid).unwrap();
        assert_eq!(component.decode(), long_but_valid);

        for raw in ["/".repeat(1024), "/".repeat(1_000_000)] {
            assert_eq!(
                SubjectComponent::encode(&raw),
                Err(SubjectComponentError::TooLong)
            );
        }
        assert_eq!(
            SubjectComponent::parse(&"a".repeat(MAX_ENCODED_COMPONENT_BYTES + 1)),
            Err(SubjectComponentError::TooLong)
        );
    }
}
