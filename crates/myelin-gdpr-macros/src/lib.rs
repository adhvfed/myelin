use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

#[proc_macro_derive(PersonalData, attributes(personal_data))]
pub fn derive_personal_data(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

const PII_FIELDS: &[&str] = &[
    "email",
    "name",
    "phone",
    "address",
    "ip_addr",
    "ip_address",
    "full_name",
    "given_name",
    "family_name",
    "display_name",
    "dob",
    "birth",
    "ssn",
    "passport",
    "body",
    "message_body",
    "comment_text",
];

fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let struct_name = &input.ident;
    let struct_name_str = struct_name.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Ok(empty_impl(
                    struct_name,
                    &impl_generics,
                    &ty_generics,
                    where_clause,
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                struct_name,
                "#[derive(PersonalData)] applies to a struct with named fields (a schema row); an \
                 enum/union has no field to classify",
            ));
        }
    };

    let mut entries: Vec<TokenStream2> = Vec::new();
    for field in fields {
        let field_ident = field
            .ident
            .as_ref()
            .expect("named field has an ident (Fields::Named)");
        let field_name = field_ident.to_string();

        let tag_attr = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("personal_data"));

        match tag_attr {
            Some(attr) => {
                let tags = parse_personal_data_tags(attr)?;
                entries.push(registry_entry(&struct_name_str, &field_name, &tags));
            }
            None => {
                if is_pii_field(&field_name) {
                    return Err(syn::Error::new_spanned(
                        field_ident,
                        format!(
                            "PII field `{field_name}` is not `#[personal_data(...)]`-tagged - a \
                             personal-data field deriving PersonalData MUST carry the five-tag \
                             classification (category/role/basis/retention/erasure/subject_locator; \
                             gdpr §2.1). An untagged PII column is the un-erasable / un-mapped \
                             subject bug class (ADR-12); tag it or it escapes the crypto-shred + \
                             RoPA fan-out."
                        ),
                    ));
                }
            }
        }
    }

    let n = entries.len();
    Ok(quote! {
        impl #impl_generics ::myelin_gdpr::HasPersonalData for #struct_name #ty_generics #where_clause {
            fn personal_data_fields() -> &'static [::myelin_gdpr::PersonalDataField] {
                const FIELDS: [::myelin_gdpr::PersonalDataField; #n] = [ #( #entries ),* ];
                &FIELDS
            }
        }
    })
}

fn empty_impl(
    struct_name: &syn::Ident,
    impl_generics: &syn::ImplGenerics<'_>,
    ty_generics: &syn::TypeGenerics<'_>,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream2 {
    quote! {
        impl #impl_generics ::myelin_gdpr::HasPersonalData for #struct_name #ty_generics #where_clause {
            fn personal_data_fields() -> &'static [::myelin_gdpr::PersonalDataField] {
                &[]
            }
        }
    }
}

fn is_pii_field(name: &str) -> bool {
    PII_FIELDS.contains(&name)
}

struct ParsedTags {
    category: String,
    role: String,
    basis: String,
    retention: String,
    erasure: String,
    subject_locator: String,
    data_role_default: String,
}

fn parse_personal_data_tags(attr: &syn::Attribute) -> syn::Result<ParsedTags> {
    let mut category: Option<String> = None;
    let mut role: Option<String> = None;
    let mut basis: Option<String> = None;
    let mut retention: Option<String> = None;
    let mut erasure: Option<String> = None;
    let mut subject_locator: Option<String> = None;
    let mut data_role_default: Option<String> = None;

    attr.parse_nested_meta(|meta| {
        let key = meta
            .path
            .get_ident()
            .map(|i| i.to_string())
            .ok_or_else(|| meta.error("each #[personal_data(...)] tag is a `key = value`"))?;

        let value = meta.value()?;
        let text = if value.peek(syn::LitStr) {
            let s: syn::LitStr = value.parse()?;
            s.value()
        } else {
            let mut collected = TokenStream2::new();
            while !value.is_empty() && !value.peek(syn::Token![,]) {
                let tt: proc_macro2::TokenTree = value.parse()?;
                collected.extend(std::iter::once(tt));
            }
            collected
                .to_string()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };

        match key.as_str() {
            "category" => category = Some(text),
            "role" => role = Some(text),
            "basis" => basis = Some(text),
            "retention" => retention = Some(text),
            "erasure" => erasure = Some(text),
            "subject_locator" => subject_locator = Some(text),
            "data_role_default" => data_role_default = Some(text),
            _ => {}
        }
        Ok(())
    })?;

    let require = |opt: Option<String>, key: &str| -> syn::Result<String> {
        opt.ok_or_else(|| {
            syn::Error::new_spanned(
                attr,
                format!(
                    "#[personal_data(...)] is missing the `{key}` tag - the five-tag classification \
                     (category/role/basis/retention/erasure) + subject_locator are ALL required \
                     (gdpr §2.1); a half-classified field is the un-mapped-subject bug class"
                ),
            )
        })
    };

    Ok(ParsedTags {
        category: require(category, "category")?,
        role: require(role, "role")?,
        basis: require(basis, "basis")?,
        retention: require(retention, "retention")?,
        erasure: require(erasure, "erasure")?,
        subject_locator: require(subject_locator, "subject_locator")?,
        data_role_default: data_role_default.unwrap_or_else(|| "Default".to_string()),
    })
}

fn registry_entry(struct_name: &str, field_name: &str, tags: &ParsedTags) -> TokenStream2 {
    let category = &tags.category;
    let role = &tags.role;
    let basis = &tags.basis;
    let retention = &tags.retention;
    let erasure = &tags.erasure;
    let subject_locator = &tags.subject_locator;
    let data_role_default = &tags.data_role_default;
    quote! {
        ::myelin_gdpr::PersonalDataField {
            owning_struct: #struct_name,
            field: #field_name,
            tags: ::myelin_gdpr::PersonalDataTags {
                category: #category,
                role: #role,
                basis: #basis,
                retention: #retention,
                erasure: #erasure,
                subject_locator: #subject_locator,
                data_role_default: #data_role_default,
            },
        }
    }
}
