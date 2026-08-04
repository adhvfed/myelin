use crate::{DataRole, PrincipalId};
use myelin_tenancy::ArtifactRef;

pub const IDENTITY_TUPLE_WRITTEN: &str = "identity.tuple.written";

pub const IDENTITY_ROLE_GRANTED: &str = "identity.role.granted";

pub const IDENTITY_BREAK_GLASS: &str = "identity.break_glass.invoked";

pub const IDENTITY_EVENT_TOKENS: &[&str] = &[
    IDENTITY_TUPLE_WRITTEN,
    IDENTITY_ROLE_GRANTED,
    IDENTITY_BREAK_GLASS,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IamSubjectRef {
    Principal(PrincipalId),
    Object(ArtifactRef),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IamEventProjection {
    pub type_: &'static str,
    pub actor_principal_id: PrincipalId,
    pub subject: IamSubjectRef,
    pub contains_personal_data: bool,
    pub data_role: DataRole,
}

impl IamEventProjection {
    pub fn new(
        type_: &'static str,
        actor_principal_id: PrincipalId,
        subject: IamSubjectRef,
        data_role: DataRole,
    ) -> Self {
        debug_assert!(
            IDENTITY_EVENT_TOKENS.contains(&type_),
            "identity.* projection built for an unregistered token: {type_}"
        );
        IamEventProjection {
            type_,
            actor_principal_id,
            subject,
            contains_personal_data: false,
            data_role,
        }
    }
}

pub mod signals {
    pub const AUTH_DECISION_LATENCY: &str = "auth_decision_latency";
    pub const CACHE_HIT_RATIO: &str = "cache_hit_ratio";
    pub const STALENESS_AGE: &str = "staleness_age";
    pub const REVOCATION_LAG: &str = "revocation_lag";
    pub const TUPLE_WRITE_LAG: &str = "tuple_write_lag";
    pub const REVERSE_INDEX_LAG: &str = "reverse_index_lag";

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

    #[test]
    fn identity_tokens_obey_the_dotted_grammar() {
        for tok in IDENTITY_EVENT_TOKENS {
            let parts: Vec<&str> = tok.split('.').collect();
            assert_eq!(
                parts.len(),
                3,
                "token `{tok}` must be <subsystem>.<artifact_type>.<event_name>"
            );
            assert_eq!(
                parts[0], "identity",
                "token `{tok}` must carry the canonical `identity` subsystem prefix (Bus §6.2)"
            );
            for seg in &parts {
                assert!(!seg.is_empty(), "token `{tok}` has an empty segment");
                assert!(
                    seg.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                    "token `{tok}` segment `{seg}` must be lowercase snake"
                );
            }
        }
        assert_eq!(IDENTITY_EVENT_TOKENS.len(), 3);
        assert!(IDENTITY_EVENT_TOKENS.contains(&IDENTITY_TUPLE_WRITTEN));
        assert!(IDENTITY_EVENT_TOKENS.contains(&IDENTITY_ROLE_GRANTED));
        assert!(IDENTITY_EVENT_TOKENS.contains(&IDENTITY_BREAK_GLASS));
    }

    #[test]
    fn iam_projection_attributes_by_opaque_id_and_classifies_correctly() {
        let tw = IamEventProjection::new(
            IDENTITY_TUPLE_WRITTEN,
            PrincipalId("p-admin".into()),
            IamSubjectRef::Object(ArtifactRef("myelin://acme/git/repo/core".into())),
            DataRole::Controller,
        );
        assert_eq!(tw.type_, IDENTITY_TUPLE_WRITTEN);
        assert_eq!(tw.actor_principal_id, PrincipalId("p-admin".into()));
        assert!(matches!(tw.subject, IamSubjectRef::Object(_)));
        assert!(
            !tw.contains_personal_data,
            "an iam.* event never carries inline PII"
        );

        let rg = IamEventProjection::new(
            IDENTITY_ROLE_GRANTED,
            PrincipalId("p-admin".into()),
            IamSubjectRef::Principal(PrincipalId("p-grantee".into())),
            DataRole::Controller,
        );
        assert_eq!(rg.type_, IDENTITY_ROLE_GRANTED);
        assert!(matches!(rg.subject, IamSubjectRef::Principal(ref id) if id.0 == "p-grantee"));
        assert!(!rg.contains_personal_data);

        let bg = IamEventProjection::new(
            IDENTITY_BREAK_GLASS,
            PrincipalId("p-oncall".into()),
            IamSubjectRef::Principal(PrincipalId("p-target".into())),
            DataRole::Controller,
        );
        assert_eq!(bg.type_, IDENTITY_BREAK_GLASS);
        assert!(!bg.contains_personal_data);
    }

    #[test]
    fn every_iam_projection_is_personal_data_free() {
        for tok in IDENTITY_EVENT_TOKENS {
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

    #[test]
    fn no_iam_projection_carries_a_pii_field() {
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
        for marker in [
            "pub struct IamEventProjection {",
            "pub enum IamSubjectRef {",
        ] {
            let start = src
                .find(marker)
                .expect("projection type is defined in this module");
            let body = &src[start..];
            let end = body.find('}').expect("type body is brace-closed");
            for line in body[..end].lines() {
                let trimmed = line.trim();
                if let Some((lhs, _)) = trimmed.split_once(':') {
                    let ident = lhs.trim_start_matches("pub ").trim();
                    let ident = ident.split(['(', ' ']).next().unwrap_or(ident);
                    assert!(
                        !PII_FIELDS.contains(&ident),
                        "iam.* projection carries forbidden PII field `{ident}` - \
                         attribution must be opaque-id-only (EI-04 §1; control-plane-pii-free)"
                    );
                }
            }
        }
    }

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
        assert!(IDENTITY_SIGNAL_NAMES.contains(&REVERSE_INDEX_LAG));
    }
}
