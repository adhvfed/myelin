use super::*;

const MAX_LINK_NOTE_BYTES: usize = 4 * 1024;
const LINK_SAVE_ATTEMPTS: usize = 4;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeLinkRequest {
    pub reference: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeLinkOutcome {
    pub page_id: String,
    pub page_ref: String,
    pub block_id: String,
    pub block_ref: String,
    pub version: i64,
    pub created: bool,
}

impl DurableKnowledgeMutationApi {
    pub fn link_work(
        &self,
        actor: &Principal,
        owner: &Principal,
        page_id: &str,
        request: KnowledgeLinkRequest,
        idempotency_key: &str,
    ) -> Result<KnowledgeLinkOutcome, EdgeError> {
        validate_link_principals(actor, owner)?;
        validate_ulid(page_id)?;
        if idempotency_key.is_empty() || idempotency_key.len() > 512 {
            return Err(EdgeError::BadRequest(
                "Knowledge link idempotency identity must be 1-512 bytes".into(),
            ));
        }
        let markdown = link_markdown(request.note.as_deref())?;
        let draft = BlockBody {
            id: None,
            block_type: "paragraph".into(),
            markdown: markdown.clone(),
            references: vec![request.reference],
            state: active_block_state(),
        };
        let references = validate_document(owner, std::slice::from_ref(&draft))?
            .into_iter()
            .next()
            .expect("one validated link block produces one reference list");
        let block_id = stable_link_block_id(
            &actor.tenant.0,
            &actor.principal_id.0,
            page_id,
            idempotency_key,
        );
        let owner_id = self.viewer(owner);
        let actor_id = self.viewer(actor);

        for _ in 0..LINK_SAVE_ATTEMPTS {
            let current = self.drive(self.store().get_visible(
                &owner.tenant.0,
                &owner.region.0,
                &owner_id,
                page_id,
            ))?;
            if current.owner != owner_id {
                return Err(EdgeError::NotFound("Knowledge page not found".into()));
            }
            if let Some(outcome) =
                replayed_link(self.kms(), &current, &block_id, &markdown, &references)?
            {
                return Ok(outcome);
            }
            validate_link_capacity(self.kms(), &current, markdown.len())?;
            let linked = KnowledgeBlockRecord {
                block_id: block_id.clone(),
                block_type: "paragraph".into(),
                inline: seal(
                    self.kms(),
                    actor,
                    &actor_id,
                    &block_scope(page_id, &block_id),
                    markdown.as_bytes(),
                )?,
                references: references.clone(),
                created_by: actor_id.clone(),
                edited_by: actor_id.clone(),
            };
            let mut blocks = current.blocks.clone();
            blocks.push(linked);
            let event_actor = pseudonymized_event_principal(&actor.tenant.0, actor);
            match self.drive(self.store().save(
                &SaveKnowledgePage {
                    tenant: owner.tenant.0.clone(),
                    region: owner.region.0.clone(),
                    page_id: page_id.to_string(),
                    owner: owner_id.clone(),
                    expected_version: current.version,
                    title: current.title,
                    visibility: current.visibility,
                    blocks,
                },
                EventId(self.mint_id()),
                Actor(event_actor),
                now_timestamp(),
            )) {
                Ok(version) => {
                    return Ok(link_outcome(
                        &owner.tenant,
                        page_id,
                        &block_id,
                        version,
                        true,
                    ))
                }
                Err(EdgeError::Conflict(_)) => continue,
                Err(error) => return Err(error),
            }
        }

        let current = self.drive(self.store().get_visible(
            &owner.tenant.0,
            &owner.region.0,
            &owner_id,
            page_id,
        ))?;
        if let Some(outcome) =
            replayed_link(self.kms(), &current, &block_id, &markdown, &references)?
        {
            return Ok(outcome);
        }
        Err(EdgeError::Conflict(
            "Knowledge page kept changing; retry this link with the same idempotency key".into(),
        ))
    }
}

fn validate_link_principals(actor: &Principal, owner: &Principal) -> Result<(), EdgeError> {
    if actor.tenant != owner.tenant || actor.region != owner.region {
        return Err(EdgeError::Forbidden(
            "Knowledge links cannot cross a tenant or region boundary".into(),
        ));
    }
    Ok(())
}

fn link_markdown(note: Option<&str>) -> Result<String, EdgeError> {
    let note = note.unwrap_or("Related work:");
    if note.is_empty()
        || note.trim() != note
        || note.len() > MAX_LINK_NOTE_BYTES
        || note.chars().any(char::is_control)
        || note.contains(myelin_content::OBJ)
    {
        return Err(EdgeError::BadRequest(format!(
            "Knowledge link note must be 1-{MAX_LINK_NOTE_BYTES} clean UTF-8 bytes without surrounding whitespace"
        )));
    }
    Ok(format!("{note} {}", myelin_content::OBJ))
}

fn stable_link_block_id(tenant: &str, actor: &str, page_id: &str, idempotency_key: &str) -> String {
    let mut digest = blake3::Hasher::new();
    for part in [
        b"myelin.knowledge.link-block.v1".as_slice(),
        tenant.as_bytes(),
        actor.as_bytes(),
        page_id.as_bytes(),
        idempotency_key.as_bytes(),
    ] {
        digest.update(&(part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    let mut value = u128::from_be_bytes(
        digest.finalize().as_bytes()[..16]
            .try_into()
            .expect("a blake3 digest has at least sixteen bytes"),
    );
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut encoded = [0_u8; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec()).expect("the Crockford alphabet is ASCII")
}

fn replayed_link(
    kms: &KmsEngine,
    page: &KnowledgePageRecord,
    block_id: &str,
    markdown: &str,
    references: &[ArtifactRef],
) -> Result<Option<KnowledgeLinkOutcome>, EdgeError> {
    let Some(block) = page.blocks.iter().find(|block| block.block_id == block_id) else {
        return Ok(None);
    };
    let visible = open_visible(
        kms,
        page,
        &block.inline,
        &block.edited_by,
        &block_scope(&page.page_id, block_id),
    )?;
    if block.block_type != "paragraph"
        || block.references != references
        || visible.as_deref() != Some(markdown.as_bytes())
    {
        return Err(EdgeError::Conflict(
            "this idempotency key already identifies a different Knowledge link".into(),
        ));
    }
    Ok(Some(link_outcome(
        &TenantId(page.tenant.clone()),
        &page.page_id,
        block_id,
        page.version,
        false,
    )))
}

fn validate_link_capacity(
    kms: &KmsEngine,
    page: &KnowledgePageRecord,
    added_markdown_bytes: usize,
) -> Result<(), EdgeError> {
    if page.blocks.len() >= MAX_BLOCKS {
        return Err(EdgeError::BadRequest(format!(
            "Knowledge page already contains the maximum of {MAX_BLOCKS} blocks"
        )));
    }
    let references = page.blocks.iter().fold(0usize, |count, block| {
        count.saturating_add(block.references.len())
    });
    if references >= MAX_PAGE_REFERENCES {
        return Err(EdgeError::BadRequest(format!(
            "Knowledge page already contains the maximum of {MAX_PAGE_REFERENCES} structured references"
        )));
    }
    let mut document_bytes = added_markdown_bytes;
    for block in &page.blocks {
        let visible = open_visible(
            kms,
            page,
            &block.inline,
            &block.edited_by,
            &block_scope(&page.page_id, &block.block_id),
        )?;
        document_bytes = document_bytes.saturating_add(visible.as_deref().map_or(0, <[u8]>::len));
        if document_bytes > MAX_DOCUMENT_BYTES {
            return Err(EdgeError::PayloadTooLarge(
                "Knowledge document exceeds 256 KiB".into(),
            ));
        }
    }
    Ok(())
}

fn link_outcome(
    tenant: &TenantId,
    page_id: &str,
    block_id: &str,
    version: i64,
    created: bool,
) -> KnowledgeLinkOutcome {
    KnowledgeLinkOutcome {
        page_id: page_id.to_string(),
        page_ref: page_ref(tenant, page_id).0,
        block_id: block_id.to_string(),
        block_ref: block_ref(tenant, page_id, block_id).0,
        version,
        created,
    }
}

pub(super) fn link_outcome_json(outcome: &KnowledgeLinkOutcome) -> Value {
    json!({
        "linked": outcome.created,
        "durable": true,
        "page_id": outcome.page_id,
        "page_ref": outcome.page_ref,
        "block_id": outcome.block_id,
        "block_ref": outcome.block_ref,
        "version": outcome.version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    #[test]
    fn link_identities_are_stable_canonical_and_scoped_to_the_retrying_actor() {
        let page = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let first = stable_link_block_id("acme", "alice", page, "retry-1");

        assert_eq!(
            first,
            stable_link_block_id("acme", "alice", page, "retry-1")
        );
        assert!(validate_ulid(&first).is_ok());
        assert_ne!(
            first,
            stable_link_block_id("acme", "agent:reviewer", page, "retry-1")
        );
        assert_ne!(
            first,
            stable_link_block_id("acme", "alice", page, "retry-2")
        );
    }

    #[test]
    fn delegated_links_never_cross_the_owners_scope() {
        let owner = principal();
        let mut actor = principal();
        assert!(validate_link_principals(&actor, &owner).is_ok());

        actor.tenant = TenantId("other".into());
        assert!(matches!(
            validate_link_principals(&actor, &owner),
            Err(EdgeError::Forbidden(_))
        ));

        actor.tenant = owner.tenant.clone();
        actor.region = Region("other-region".into());
        assert!(matches!(
            validate_link_principals(&actor, &owner),
            Err(EdgeError::Forbidden(_))
        ));
    }

    #[test]
    fn link_notes_create_exactly_one_positional_reference() {
        let target = "myelin://acme/issue/issue/ENG-1";
        for note in [None, Some("Implements the acceptance criteria:")] {
            let markdown = link_markdown(note).expect("clean link note");
            assert_eq!(markdown.matches(myelin_content::OBJ).count(), 1);
            let block = BlockBody {
                id: None,
                block_type: "paragraph".into(),
                markdown,
                references: vec![target.into()],
                state: active_block_state(),
            };
            assert_eq!(
                validate_document(&principal(), &[block]).unwrap(),
                vec![vec![ArtifactRef(target.into())]]
            );
        }

        for note in [
            Some(""),
            Some(" padded"),
            Some("line\nbreak"),
            Some("bad ￼"),
        ] {
            assert!(link_markdown(note).is_err());
        }
    }
}
