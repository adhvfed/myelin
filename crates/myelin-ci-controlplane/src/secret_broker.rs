use myelin_ci_sandbox::{JobSpec, ResolvedSecretEnv, SecretInjectionError, SecretRef, TrustTier};
use myelin_identity::{
    Consistency, Decision, IdentityService, Permission, Principal, Result as IdResult,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::fmt;
use zeroize::Zeroizing;

pub const SECRET_READ_PERMISSION: &str = "read";

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSecret {
    pub name: String,
    pub value: Zeroizing<String>,
}

impl fmt::Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSecret")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithholdReason {
    UntrustedFork,
    NotGranted,
    CapabilityUnavailable,
}

impl WithholdReason {
    pub fn as_token(self) -> &'static str {
        match self {
            WithholdReason::UntrustedFork => "untrusted_fork",
            WithholdReason::NotGranted => "not_granted",
            WithholdReason::CapabilityUnavailable => "capability_unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretOutcome {
    Resolved(ResolvedSecret),
    Withheld {
        name: String,
        reason: WithholdReason,
    },
}

impl SecretOutcome {
    pub fn resolved(&self) -> Option<&ResolvedSecret> {
        match self {
            SecretOutcome::Resolved(r) => Some(r),
            SecretOutcome::Withheld { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretResolution {
    pub outcomes: Vec<SecretOutcome>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithheldSecret {
    pub name: String,
    pub reason: WithholdReason,
}

#[derive(Debug)]
pub enum SecretLaunchError {
    Authorization(myelin_identity::AuthzError),
    Withheld(Vec<WithheldSecret>),
    Injection(SecretInjectionError),
}

impl fmt::Display for SecretLaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authorization(_) => formatter.write_str("secret launch authorization failed"),
            Self::Withheld(withheld) => {
                formatter.write_str("secret launch withheld: ")?;
                for (index, item) in withheld.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(",")?;
                    }
                    write!(formatter, "{}={}", item.name, item.reason.as_token())?;
                }
                Ok(())
            }
            Self::Injection(error) => write!(formatter, "secret launch injection refused: {error}"),
        }
    }
}

impl std::error::Error for SecretLaunchError {}

impl SecretResolution {
    pub fn secret_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.resolved().is_some())
            .count()
    }

    pub fn resolved(&self) -> impl Iterator<Item = &ResolvedSecret> {
        self.outcomes.iter().filter_map(SecretOutcome::resolved)
    }

    pub fn all_resolved(&self) -> bool {
        self.outcomes.iter().all(|o| o.resolved().is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.secret_count() == 0
    }
}

pub trait SecretCapability {
    fn resolve_handle(
        &self,
        tenant: &TenantId,
        object: &ArtifactRef,
        binding_name: &str,
        handle: &str,
    ) -> Option<Zeroizing<String>>;
}

const MAX_SECRET_HANDLE_SEGMENT_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanonicalSecretHandle<'a> {
    pub tenant: &'a str,
    pub id: &'a str,
}

pub(crate) fn parse_canonical_secret_handle(handle: &str) -> Option<CanonicalSecretHandle<'_>> {
    let rest = handle.strip_prefix("myelin://")?;
    let mut segments = rest.split('/');
    let tenant = segments.next()?;
    let subsystem = segments.next()?;
    let object_type = segments.next()?;
    let id = segments.next()?;
    if segments.next().is_some()
        || subsystem != "ci"
        || object_type != "secret"
        || !strict_secret_segment(tenant)
        || !strict_secret_segment(id)
    {
        return None;
    }

    let canonical = format!("myelin://{tenant}/ci/secret/{id}");
    if handle != canonical {
        return None;
    }
    let key = myelin_refs::object_key(&ArtifactRef(canonical))?;
    if key.tenant.as_deref() != Some(tenant)
        || key.subsystem.as_deref() != Some("ci")
        || key.object_type.as_deref() != Some("secret")
        || key.id != id
        || key.tuple_key() != format!("secret:{id}")
    {
        return None;
    }

    Some(CanonicalSecretHandle { tenant, id })
}

pub(crate) fn strict_secret_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_SECRET_HANDLE_SEGMENT_BYTES
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn tenant_bound_secret_object(
    tenant: &TenantId,
    sref: &SecretRef,
    secret_object_of: &impl Fn(&SecretRef) -> ArtifactRef,
) -> Option<ArtifactRef> {
    let parsed = parse_canonical_secret_handle(&sref.handle)?;
    if parsed.tenant != tenant.0 {
        return None;
    }
    let object = secret_object_of(sref);
    let authorized = parse_canonical_secret_handle(&object.0)?;
    (authorized == parsed && object.0 == sref.handle).then_some(object)
}

#[derive(Clone, PartialEq, Eq)]
pub struct OidcCredential {
    pub audience: String,
    pub token: String,
    pub ttl_secs: u32,
}

impl fmt::Debug for OidcCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OidcCredential")
            .field("audience", &self.audience)
            .field("token", &"[REDACTED]")
            .field("ttl_secs", &self.ttl_secs)
            .finish()
    }
}

pub struct SecretBroker<'a, C: SecretCapability, I: IdentityService> {
    cap: &'a C,
    identity: &'a I,
}

impl<'a, C: SecretCapability, I: IdentityService> SecretBroker<'a, C, I> {
    pub fn new(cap: &'a C, identity: &'a I) -> Self {
        SecretBroker { cap, identity }
    }

    pub fn resolve(
        &self,
        tier: TrustTier,
        subject: &Principal,
        secret_object_of: impl Fn(&SecretRef) -> ArtifactRef,
        secret_refs: &[SecretRef],
        at: &Consistency,
    ) -> IdResult<SecretResolution> {
        if tier == TrustTier::UntrustedFork {
            return Ok(SecretResolution {
                outcomes: secret_refs
                    .iter()
                    .map(|r| SecretOutcome::Withheld {
                        name: r.name.clone(),
                        reason: WithholdReason::UntrustedFork,
                    })
                    .collect(),
            });
        }

        let mut outcomes = Vec::with_capacity(secret_refs.len());
        for sref in secret_refs {
            let Some(object) = tenant_bound_secret_object(&subject.tenant, sref, &secret_object_of)
            else {
                outcomes.push(SecretOutcome::Withheld {
                    name: sref.name.clone(),
                    reason: WithholdReason::NotGranted,
                });
                continue;
            };
            let decision = self.identity.check(
                subject,
                &Permission(SECRET_READ_PERMISSION.to_string()),
                &object,
                at,
                None,
            )?;
            if decision == Decision::Allow {
                match self
                    .cap
                    .resolve_handle(&subject.tenant, &object, &sref.name, &sref.handle)
                {
                    Some(value) => outcomes.push(SecretOutcome::Resolved(ResolvedSecret {
                        name: sref.name.clone(),
                        value,
                    })),
                    None => outcomes.push(SecretOutcome::Withheld {
                        name: sref.name.clone(),
                        reason: WithholdReason::NotGranted,
                    }),
                }
            } else {
                outcomes.push(SecretOutcome::Withheld {
                    name: sref.name.clone(),
                    reason: WithholdReason::NotGranted,
                });
            }
        }
        Ok(SecretResolution { outcomes })
    }

    pub fn resolve_for_launch(
        &self,
        spec: JobSpec,
        subject: &Principal,
        secret_object_of: impl Fn(&SecretRef) -> ArtifactRef,
        at: &Consistency,
    ) -> Result<JobSpec, SecretLaunchError> {
        let resolution = self
            .resolve(
                spec.trust_tier,
                subject,
                secret_object_of,
                &spec.secret_refs,
                at,
            )
            .map_err(SecretLaunchError::Authorization)?;

        let withheld: Vec<WithheldSecret> = resolution
            .outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                SecretOutcome::Resolved(_) => None,
                SecretOutcome::Withheld { name, reason } => Some(WithheldSecret {
                    name: name.clone(),
                    reason: *reason,
                }),
            })
            .collect();
        if !withheld.is_empty() {
            return Err(SecretLaunchError::Withheld(withheld));
        }

        let bindings = resolution
            .outcomes
            .into_iter()
            .filter_map(|outcome| match outcome {
                SecretOutcome::Resolved(secret) => Some(ResolvedSecretEnv::from_zeroizing(
                    secret.name,
                    secret.value,
                )),
                SecretOutcome::Withheld { .. } => None,
            })
            .collect();
        spec.with_resolved_secrets(bindings)
            .map_err(SecretLaunchError::Injection)
    }

    pub fn mint_oidc(
        &self,
        tier: TrustTier,
        audience: &str,
        ttl_secs: u32,
        mint: impl FnOnce(&str, u32) -> Option<String>,
    ) -> Option<OidcCredential> {
        if tier == TrustTier::UntrustedFork {
            return None;
        }
        mint(audience, ttl_secs).map(|token| OidcCredential {
            audience: audience.to_string(),
            token,
            ttl_secs,
        })
    }
}

#[cfg(test)]
#[path = "secret_broker_tests.rs"]
mod tests;
