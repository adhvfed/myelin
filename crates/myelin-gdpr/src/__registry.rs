use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalDataField {
    pub owning_struct: &'static str,
    pub field: &'static str,
    pub tags: PersonalDataTags,
}

impl PersonalDataField {
    pub fn erasure_key_class(&self) -> Option<ErasureKeyClass> {
        ErasureKeyClass::from_erasure_tag(self.tags.erasure)
    }

    pub fn is_special_category(&self) -> Option<SpecialCategoryFlag> {
        SpecialCategoryFlag::from_category_tag(self.tags.category)
    }

    pub fn data_role_default(&self) -> DataRoleDefault {
        DataRoleDefault::from_tag(self.tags.data_role_default)
    }

    pub fn is_restricted_by_default(&self) -> bool {
        self.data_role_default() == DataRoleDefault::Restricted
    }

    pub fn is_behavioural(&self) -> bool {
        self.tags.category == "Behavioural"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalDataTags {
    pub category: &'static str,
    pub role: &'static str,
    pub basis: &'static str,
    pub retention: &'static str,
    pub erasure: &'static str,
    pub subject_locator: &'static str,
    #[serde(default = "default_data_role_default")]
    pub data_role_default: &'static str,
}

pub const fn default_data_role_default() -> &'static str {
    "Default"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRoleDefault {
    Default,
    Restricted,
}

impl DataRoleDefault {
    pub fn from_tag(text: &str) -> DataRoleDefault {
        match text {
            "Restricted" => DataRoleDefault::Restricted,
            _ => DataRoleDefault::Default,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErasureKeyClass {
    SubjectDek,
    TenantKek,
    Other(&'static str),
}

impl ErasureKeyClass {
    pub fn from_erasure_tag(erasure: &'static str) -> Option<ErasureKeyClass> {
        let inner = erasure
            .strip_prefix("CryptoShred(")
            .and_then(|s| s.strip_suffix(')'))?
            .trim();
        Some(match inner {
            "subject_dek" => ErasureKeyClass::SubjectDek,
            "tenant_kek" => ErasureKeyClass::TenantKek,
            other => ErasureKeyClass::Other(other),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialCategoryFlag {
    pub kind: &'static str,
}

impl SpecialCategoryFlag {
    pub fn from_category_tag(category: &'static str) -> Option<SpecialCategoryFlag> {
        let kind = category
            .strip_prefix("SpecialCategory(")
            .and_then(|s| s.strip_suffix(')'))?
            .trim();
        Some(SpecialCategoryFlag { kind })
    }
}

pub trait HasPersonalData {
    fn personal_data_fields() -> &'static [PersonalDataField];

    fn subject_locator(field: &str) -> Option<&'static str> {
        Self::personal_data_fields()
            .iter()
            .find(|f| f.field == field)
            .map(|f| f.tags.subject_locator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erasure_key_class_parses_the_crypto_shred_payload() {
        assert_eq!(
            ErasureKeyClass::from_erasure_tag("CryptoShred(subject_dek)"),
            Some(ErasureKeyClass::SubjectDek)
        );
        assert_eq!(
            ErasureKeyClass::from_erasure_tag("CryptoShred(tenant_kek)"),
            Some(ErasureKeyClass::TenantKek)
        );
        assert_eq!(
            ErasureKeyClass::from_erasure_tag("CryptoShred(custom)"),
            Some(ErasureKeyClass::Other("custom"))
        );
        assert_eq!(ErasureKeyClass::from_erasure_tag("Pseudonymise"), None);
        assert_eq!(ErasureKeyClass::from_erasure_tag("PurgeReindex"), None);
        assert_eq!(ErasureKeyClass::from_erasure_tag("CarveOut"), None);
    }

    #[test]
    fn special_category_flag_parses_the_art9_kind() {
        assert_eq!(
            SpecialCategoryFlag::from_category_tag("SpecialCategory(health)"),
            Some(SpecialCategoryFlag { kind: "health" })
        );
        assert_eq!(SpecialCategoryFlag::from_category_tag("ContactInfo"), None);
        assert_eq!(SpecialCategoryFlag::from_category_tag("Behavioural"), None);
    }
}
