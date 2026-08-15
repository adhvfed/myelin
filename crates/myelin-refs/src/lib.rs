mod edge;
pub mod git_coordinate;
mod object_key;
mod parse;

use myelin_identity::Principal;
use serde::{Deserialize, Serialize};

pub use myelin_events::{AggregateKey, ArtifactRef};

pub use edge::{
    identity_member_ref, reference_edge_draft, EdgeChange, ReferenceRel, REFS_EDGE_CREATED,
    REFS_EDGE_REMOVED, REL_CLASS_REFERENCE,
};

pub use parse::{
    format, mint, parse, parse_scoped, strip_sub, sub_kind, ParseError, ParsedArtifactRef, Sub,
    SubKind, SCHEME,
};

pub use object_key::{object_key, ObjectKey};

/// A bounded, privacy-preserving ordering key for all events about one directed edge.
///
/// Artifact refs are intentionally absent from the broker subject derived from this aggregate.
/// Apart from avoiding metadata disclosure, hashing keeps long `#sub` references inside the event
/// envelope instead of exceeding the event-stream token limit.
pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    let mut hash = blake3::Hasher::new();
    hash.update(b"myelin.refs.edge.aggregate.v1\0");
    for value in [&source.0, &target.0] {
        hash.update(&(value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    AggregateKey(format!("refs-edge:{}", hash.finalize().to_hex()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubKindRegistration {
    pub subsystem: String,
    pub kinds: Vec<SubKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistrationError {
    UnknownSubsystem { token: String },
    NoKinds,
    DuplicateKind { kind: &'static str },
}

impl core::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RegistrationError::UnknownSubsystem { token } => write!(
                f,
                "unknown subsystem token `{token}`: a `#sub` mint owner must be a canonical Bus \
                 §6.2 subsystem token - Refs validates against the Bus table, it never authors one."
            ),
            RegistrationError::NoKinds => write!(
                f,
                "empty `#sub` registration: a subsystem must claim at least one `#sub` kind to \
                 register ownership (an empty claim is meaningless)."
            ),
            RegistrationError::DuplicateKind { kind } => write!(
                f,
                "ambiguous `#sub` registration: kind `{kind}` is claimed twice in one registration."
            ),
        }
    }
}

impl std::error::Error for RegistrationError {}

impl SubKindRegistration {
    pub fn validate(self) -> core::result::Result<Self, RegistrationError> {
        if self.kinds.is_empty() {
            return Err(RegistrationError::NoKinds);
        }
        if !myelin_events::SUBSYSTEM_TOKENS.contains(&self.subsystem.as_str()) {
            return Err(RegistrationError::UnknownSubsystem {
                token: self.subsystem.clone(),
            });
        }
        let mut seen: Vec<SubKind> = Vec::with_capacity(self.kinds.len());
        for &k in &self.kinds {
            if seen.contains(&k) {
                return Err(RegistrationError::DuplicateKind { kind: k.label() });
            }
            seen.push(k);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: ArtifactRef,
    pub to: ArtifactRef,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefError {
    Parse(ParseError),
    ProjectionUnavailable,
}

impl From<ParseError> for RefError {
    fn from(e: ParseError) -> Self {
        RefError::Parse(e)
    }
}

impl core::fmt::Display for RefError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RefError::Parse(e) => write!(f, "{e}"),
            RefError::ProjectionUnavailable => {
                write!(
                    f,
                    "refs edge projection is not available on the stateless codec"
                )
            }
        }
    }
}

impl std::error::Error for RefError {}

pub type Result<T> = core::result::Result<T, RefError>;

pub trait Refs {
    fn parse(s: &str) -> Result<ArtifactRef>;
    fn format(r: &ArtifactRef) -> String;
    fn edges(&self, r: &ArtifactRef) -> Result<Vec<Edge>>;
    fn backlinks(&self, r: &ArtifactRef, viewer: &Principal) -> Result<Vec<Edge>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RefsCodec;

impl Refs for RefsCodec {
    fn parse(s: &str) -> Result<ArtifactRef> {
        parse::parse(s).map_err(RefError::from)
    }

    fn format(r: &ArtifactRef) -> String {
        parse::format(r)
    }

    fn edges(&self, _r: &ArtifactRef) -> Result<Vec<Edge>> {
        Err(RefError::ProjectionUnavailable)
    }

    fn backlinks(&self, _r: &ArtifactRef, _viewer: &Principal) -> Result<Vec<Edge>> {
        Err(RefError::ProjectionUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    #[test]
    fn refs_codec_parse_format_round_trips_and_edge_methods_are_the_named_floor() {
        let s = "myelin://acme/issue/issue/ENG-1421";
        let r = <RefsCodec as Refs>::parse(s).expect("canonical URN parses");
        assert_eq!(<RefsCodec as Refs>::format(&r), s);

        let viewer = Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        let codec = RefsCodec;
        assert_eq!(codec.edges(&r), Err(RefError::ProjectionUnavailable));
        assert_eq!(
            codec.backlinks(&r, &viewer),
            Err(RefError::ProjectionUnavailable)
        );
    }

    #[test]
    fn edge_aggregate_keys_are_bounded_opaque_and_stable() {
        let source = ArtifactRef(format!(
            "myelin://acme/chat/message/{}#{}",
            "m".repeat(180),
            "block-9".repeat(20)
        ));
        let target = ArtifactRef(format!(
            "myelin://acme/knowledge/page/{}#{}",
            "p".repeat(180),
            "block-3".repeat(20)
        ));

        let first = edge_aggregate_key(&source, &target);
        let replay = edge_aggregate_key(&source, &target);
        assert_eq!(first, replay);
        assert_eq!(first.0.len(), "refs-edge:".len() + 64);
        assert!(!first.0.contains("myelin://"));
        assert_ne!(first, edge_aggregate_key(&target, &source));
    }

    #[test]
    fn sub_kind_registration_is_accepted_for_a_canonical_owner() {
        let reg = SubKindRegistration {
            subsystem: "git".into(),
            kinds: vec![SubKind::Comment, SubKind::Thread, SubKind::LineRange],
        }
        .validate()
        .expect("a canonical-owner, frozen-kind registration is accepted");
        assert_eq!(reg.subsystem, "git");
    }

    #[test]
    fn sub_kind_registration_is_rejected_loudly_for_bad_claims() {
        assert!(matches!(
            SubKindRegistration {
                subsystem: "billing".into(),
                kinds: vec![SubKind::Comment],
            }
            .validate(),
            Err(RegistrationError::UnknownSubsystem { .. })
        ));
        assert!(matches!(
            SubKindRegistration {
                subsystem: "git".into(),
                kinds: vec![],
            }
            .validate(),
            Err(RegistrationError::NoKinds)
        ));
        assert!(matches!(
            SubKindRegistration {
                subsystem: "git".into(),
                kinds: vec![SubKind::Comment, SubKind::Comment],
            }
            .validate(),
            Err(RegistrationError::DuplicateKind { .. })
        ));
        assert!(RegistrationError::NoKinds.to_string().contains("empty"));
        assert!(RegistrationError::UnknownSubsystem {
            token: "billing".into()
        }
        .to_string()
        .contains("billing"));
    }

    #[test]
    fn refs_codec_rejects_a_display_projection() {
        assert!(matches!(
            <RefsCodec as Refs>::parse("#1421"),
            Err(RefError::Parse(_))
        ));
    }
}
