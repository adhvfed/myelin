use myelin_refs::{mint, ArtifactRef, ParseError, Sub, SubKind, SubKindRegistration};

pub const KNOWLEDGE_SUBSYSTEM: &str = "knowledge";

pub const KNOWLEDGE_OWNED_SUB_KINDS: &[SubKind] = &[SubKind::Block, SubKind::Heading];

pub fn register_knowledge_sub_kinds() -> Result<SubKindRegistration, myelin_refs::RegistrationError>
{
    SubKindRegistration {
        subsystem: KNOWLEDGE_SUBSYSTEM.to_string(),
        kinds: KNOWLEDGE_OWNED_SUB_KINDS.to_vec(),
    }
    .validate()
}

fn page_root(tenant: &str, page_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/knowledge/page/{page_id}"))
}

pub fn mint_block(tenant: &str, page_id: &str, block_id: &str) -> Result<ArtifactRef, ParseError> {
    let root = page_root(tenant, page_id)?;
    mint(&root, Sub::Block(block_id.to_string()))
}

pub fn mint_heading(
    tenant: &str,
    page_id: &str,
    block_id: &str,
) -> Result<ArtifactRef, ParseError> {
    let root = page_root(tenant, page_id)?;
    mint(&root, Sub::Heading(block_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_refs::{format, strip_sub, sub_kind};

    #[test]
    fn refs_accepts_knowledge_block_and_heading_registration() {
        let reg =
            register_knowledge_sub_kinds().expect("Refs accepts Knowledge's #sub registration");
        assert_eq!(reg.subsystem, "knowledge");
        assert_eq!(reg.kinds, vec![SubKind::Block, SubKind::Heading]);
    }

    #[test]
    fn knowledge_registers_only_block_and_heading() {
        let reg = register_knowledge_sub_kinds().expect("registration accepted");
        for k in &reg.kinds {
            assert!(
                matches!(k, SubKind::Block | SubKind::Heading),
                "Knowledge registered a non-Knowledge-owned #sub kind `{k:?}`"
            );
        }
        assert!(!reg.kinds.contains(&SubKind::Comment));
        assert!(!reg.kinds.contains(&SubKind::Row));
        assert!(!reg.kinds.contains(&SubKind::Field));
    }

    #[test]
    fn block_and_heading_mints_are_grammatical_and_classify() {
        let b = mint_block("acme-eu", "7c2", "b9").expect("block mint is grammatical");
        let h = mint_heading("acme-eu", "7c2", "hdr1").expect("heading mint is grammatical");

        let rb = myelin_refs::parse(&format(&b)).expect("block ref is canonical");
        let rh = myelin_refs::parse(&format(&h)).expect("heading ref is canonical");
        assert_eq!(format(&rb), format(&b));
        assert_eq!(format(&rh), format(&h));
        assert_eq!(sub_kind(&rb).map(|s| s.kind()), Some(SubKind::Block));
        assert_eq!(sub_kind(&rh).map(|s| s.kind()), Some(SubKind::Heading));

        for r in [&b, &h] {
            let root = strip_sub(r);
            assert!(
                !format(&root).contains('#'),
                "stripped root carries a #sub: {}",
                format(&root)
            );
            assert!(
                myelin_refs::parse(&format(&root)).is_ok(),
                "stripped root must itself parse as a canonical root"
            );
        }
    }

    #[test]
    fn empty_block_id_is_rejected_loudly() {
        assert!(mint_block("acme", "p1", "").is_err());
        assert!(mint_heading("acme", "p1", "").is_err());
    }
}
