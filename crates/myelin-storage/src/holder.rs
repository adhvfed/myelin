use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OltpHolderRegistration {
    pub store: &'static str,
}

pub fn register_holder(store: &'static str) -> OltpHolderRegistration {
    OltpHolderRegistration { store }
}

#[derive(Clone, Debug)]
pub struct OltpStoreHolder {
    pub store: &'static str,
}

impl OltpStoreHolder {
    pub fn new(store: &'static str) -> OltpStoreHolder {
        OltpStoreHolder { store }
    }

    pub fn register(&self) -> OltpHolderRegistration {
        register_holder(self.store)
    }
}

fn dsr_floor(method: &str) -> DsrError {
    DsrError(format!(
        "OLTP {method} body lands in GDPR M1 (10.1-10.9); P-ST-01 ships the registration hook only"
    ))
}

impl PersonalDataHolder for OltpStoreHolder {
    fn locate(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<LocateReport> {
        Err(dsr_floor("locate"))
    }
    fn export(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<PortableBundle> {
        Err(dsr_floor("export"))
    }
    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Err(dsr_floor("rectify"))
    }
    fn restrict(&self, _subject: &SubjectRef, _on: bool) -> DsrResult<RestrictReceipt> {
        Err(dsr_floor("restrict"))
    }
    fn erase(&self, _scope: EraseScope) -> DsrResult<EraseReceipt> {
        Err(dsr_floor("erase"))
    }
}

#[derive(Clone, Debug)]
pub struct BlobStoreHolder {
    pub store: &'static str,
}

impl BlobStoreHolder {
    pub fn new(store: &'static str) -> BlobStoreHolder {
        BlobStoreHolder { store }
    }

    pub fn register(&self) -> OltpHolderRegistration {
        register_holder(self.store)
    }
}

impl PersonalDataHolder for BlobStoreHolder {
    fn locate(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<LocateReport> {
        Err(dsr_floor("blob locate"))
    }
    fn export(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<PortableBundle> {
        Err(dsr_floor("blob export"))
    }
    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Err(dsr_floor("blob rectify"))
    }
    fn restrict(&self, _subject: &SubjectRef, _on: bool) -> DsrResult<RestrictReceipt> {
        Err(dsr_floor("blob restrict"))
    }
    fn erase(&self, _scope: EraseScope) -> DsrResult<EraseReceipt> {
        Err(dsr_floor("blob erase (crypto-shred)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn subject() -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    #[test]
    fn registration_hook_fires_and_names_the_store() {
        let receipt = register_holder("issue_oltp");
        assert_eq!(receipt.store, "issue_oltp");
    }

    #[test]
    fn store_holder_registers_itself() {
        let holder = OltpStoreHolder::new("worklog_oltp");
        assert_eq!(
            holder.register(),
            OltpHolderRegistration {
                store: "worklog_oltp"
            }
        );
    }

    #[test]
    fn blob_store_registers_as_a_holder() {
        let holder = BlobStoreHolder::new("git_pack_blobs");
        assert_eq!(
            holder.register(),
            OltpHolderRegistration {
                store: "git_pack_blobs"
            }
        );
        let _s = subject();
        let scope = EraseScope::Subject {
            subject: subject(),
            tenant: tenant(),
        };
        match holder.erase(scope) {
            Err(DsrError(msg)) => assert!(msg.contains("crypto-shred")),
            Ok(_) => panic!("blob erase must be the GDPR-M1 crypto-shred floor"),
        }
    }

    #[test]
    fn dsr_bodies_are_the_named_gdpr_m1_floor() {
        let holder = OltpStoreHolder::new("issue_oltp");
        let s = subject();
        assert!(holder.locate(&s, tenant()).is_err());
        assert!(holder.erase(EraseScope::Tenant(tenant())).is_err());
        match holder.export(&s, tenant()) {
            Err(DsrError(msg)) => {
                assert!(
                    msg.contains("GDPR M1"),
                    "floor must name its follow-on: {msg}"
                )
            }
            Ok(_) => panic!("export body must be the GDPR-M1 floor on P-ST-01"),
        }
    }
}
