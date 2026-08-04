use myelin_events::validate_event_type;

pub const KNOWLEDGE_PAGE_CREATED: &str = "knowledge.page.created";
pub const KNOWLEDGE_PAGE_UPDATED: &str = "knowledge.page.updated";
pub const KNOWLEDGE_PAGE_MOVED: &str = "knowledge.page.moved";
pub const KNOWLEDGE_PAGE_ARCHIVED: &str = "knowledge.page.archived";
pub const KNOWLEDGE_PAGE_RESTORED: &str = "knowledge.page.restored";
pub const KNOWLEDGE_PAGE_DELETED: &str = "knowledge.page.deleted";
pub const KNOWLEDGE_PAGE_PUBLISHED: &str = "knowledge.page.published";
pub const KNOWLEDGE_PAGE_UNPUBLISHED: &str = "knowledge.page.unpublished";
pub const KNOWLEDGE_DOC_UPDATED: &str = "knowledge.doc.updated";
pub const KNOWLEDGE_PAGE_PARENT_SET: &str = "knowledge.page.parent_set";

pub const KNOWLEDGE_BLOCK_CREATED: &str = "knowledge.block.created";
pub const KNOWLEDGE_BLOCK_UPDATED: &str = "knowledge.block.updated";
pub const KNOWLEDGE_BLOCK_DELETED: &str = "knowledge.block.deleted";

pub const KNOWLEDGE_DATABASE_CREATED: &str = "knowledge.database.created";
pub const KNOWLEDGE_DATABASE_SCHEMA_CHANGED: &str = "knowledge.database.schema_changed";
pub const KNOWLEDGE_VIEW_CREATED: &str = "knowledge.view.created";
pub const KNOWLEDGE_VIEW_UPDATED: &str = "knowledge.view.updated";
pub const KNOWLEDGE_ROW_CREATED: &str = "knowledge.row.created";
pub const KNOWLEDGE_ROW_UPDATED: &str = "knowledge.row.updated";
pub const KNOWLEDGE_ROW_DELETED: &str = "knowledge.row.deleted";
pub const KNOWLEDGE_ROW_MOVED: &str = "knowledge.row.moved";
pub const KNOWLEDGE_RELATION_CREATED: &str = "knowledge.relation.created";
pub const KNOWLEDGE_RELATION_REMOVED: &str = "knowledge.relation.removed";

pub const KNOWLEDGE_COMMENT_CREATED: &str = "knowledge.comment.created";
pub const KNOWLEDGE_COMMENT_RESOLVED: &str = "knowledge.comment.resolved";
pub const KNOWLEDGE_MENTION_CREATED: &str = "knowledge.mention.created";

pub const KNOWLEDGE_ACCESS_GRANTED: &str = "knowledge.access.granted";
pub const KNOWLEDGE_ACCESS_REVOKED: &str = "knowledge.access.revoked";
pub const KNOWLEDGE_SUBJECT_EXPORT_REQUESTED: &str = "knowledge.subject.export_requested";
pub const KNOWLEDGE_SUBJECT_EXPORT_COMPLETED: &str = "knowledge.subject.export_completed";
pub const KNOWLEDGE_SUBJECT_ERASURE_REQUESTED: &str = "knowledge.subject.erasure_requested";
pub const KNOWLEDGE_SUBJECT_ERASURE_COMPLETED: &str = "knowledge.subject.erasure_completed";

pub const KNOWLEDGE_PAGE_ERASED: &str = "knowledge.page.erased";
pub const KNOWLEDGE_ROW_ERASED: &str = "knowledge.row.erased";
pub const KNOWLEDGE_COMMENT_ERASED: &str = "knowledge.comment.erased";

pub const KNOWLEDGE_PAGE_SNAPSHOT: &str = "knowledge.page.snapshot";
pub const KNOWLEDGE_BLOCK_SNAPSHOT: &str = "knowledge.block.snapshot";
pub const KNOWLEDGE_ROW_SNAPSHOT: &str = "knowledge.row.snapshot";

pub const KNOWLEDGE_DURABLE_TOKENS: &[&str] = &[
    KNOWLEDGE_PAGE_CREATED,
    KNOWLEDGE_PAGE_UPDATED,
    KNOWLEDGE_PAGE_MOVED,
    KNOWLEDGE_PAGE_ARCHIVED,
    KNOWLEDGE_PAGE_RESTORED,
    KNOWLEDGE_PAGE_DELETED,
    KNOWLEDGE_PAGE_PUBLISHED,
    KNOWLEDGE_PAGE_UNPUBLISHED,
    KNOWLEDGE_DOC_UPDATED,
    KNOWLEDGE_PAGE_PARENT_SET,
    KNOWLEDGE_BLOCK_CREATED,
    KNOWLEDGE_BLOCK_UPDATED,
    KNOWLEDGE_BLOCK_DELETED,
    KNOWLEDGE_DATABASE_CREATED,
    KNOWLEDGE_DATABASE_SCHEMA_CHANGED,
    KNOWLEDGE_VIEW_CREATED,
    KNOWLEDGE_VIEW_UPDATED,
    KNOWLEDGE_ROW_CREATED,
    KNOWLEDGE_ROW_UPDATED,
    KNOWLEDGE_ROW_DELETED,
    KNOWLEDGE_ROW_MOVED,
    KNOWLEDGE_RELATION_CREATED,
    KNOWLEDGE_RELATION_REMOVED,
    KNOWLEDGE_COMMENT_CREATED,
    KNOWLEDGE_COMMENT_RESOLVED,
    KNOWLEDGE_MENTION_CREATED,
    KNOWLEDGE_ACCESS_GRANTED,
    KNOWLEDGE_ACCESS_REVOKED,
    KNOWLEDGE_SUBJECT_EXPORT_REQUESTED,
    KNOWLEDGE_SUBJECT_EXPORT_COMPLETED,
    KNOWLEDGE_SUBJECT_ERASURE_REQUESTED,
    KNOWLEDGE_SUBJECT_ERASURE_COMPLETED,
    KNOWLEDGE_PAGE_ERASED,
    KNOWLEDGE_ROW_ERASED,
    KNOWLEDGE_COMMENT_ERASED,
    KNOWLEDGE_PAGE_SNAPSHOT,
    KNOWLEDGE_BLOCK_SNAPSHOT,
    KNOWLEDGE_ROW_SNAPSHOT,
];

pub const KNOWLEDGE_BLOCK_OP: &str = "knowledge.block.op";
pub const KNOWLEDGE_PRESENCE_UPDATED: &str = "knowledge.presence.updated";

pub const KNOWLEDGE_FIREHOSE_TOKENS: &[&str] = &[KNOWLEDGE_BLOCK_OP, KNOWLEDGE_PRESENCE_UPDATED];

pub fn register_knowledge_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for &tok in KNOWLEDGE_DURABLE_TOKENS
        .iter()
        .chain(KNOWLEDGE_FIREHOSE_TOKENS)
    {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_knowledge_token_parses_the_bus_grammar() {
        for &tok in KNOWLEDGE_DURABLE_TOKENS
            .iter()
            .chain(KNOWLEDGE_FIREHOSE_TOKENS)
        {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered knowledge token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        assert!(register_knowledge_tokens().is_ok());
    }

    #[test]
    fn every_knowledge_token_carries_the_knowledge_prefix() {
        for &tok in KNOWLEDGE_DURABLE_TOKENS
            .iter()
            .chain(KNOWLEDGE_FIREHOSE_TOKENS)
        {
            assert_eq!(tok.split('.').next().unwrap(), "knowledge");
        }
        assert!(myelin_events::SUBSYSTEM_TOKENS.contains(&"knowledge"));
    }

    #[test]
    fn durable_and_firehose_sets_are_disjoint() {
        for d in KNOWLEDGE_DURABLE_TOKENS {
            assert!(
                !KNOWLEDGE_FIREHOSE_TOKENS.contains(d),
                "`{d}` cannot be both durable and firehose"
            );
        }
    }

    #[test]
    fn the_knowledge_list_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for &tok in KNOWLEDGE_DURABLE_TOKENS
            .iter()
            .chain(KNOWLEDGE_FIREHOSE_TOKENS)
        {
            assert!(
                seen.insert(tok),
                "knowledge token `{tok}` registered more than once"
            );
        }
    }

    #[test]
    fn knowledge_registers_no_foreign_subsystem_tokens() {
        for &tok in KNOWLEDGE_DURABLE_TOKENS
            .iter()
            .chain(KNOWLEDGE_FIREHOSE_TOKENS)
        {
            assert!(
                tok.starts_with("knowledge."),
                "foreign-subsystem token `{tok}`"
            );
        }
    }
}
