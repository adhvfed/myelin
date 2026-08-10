use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::Method;
use myelin_events::{Actor, EventId, IdMinter, Timestamp};
use myelin_identity::Principal;
use myelin_knowledge::{
    decrypt_text, encrypt_text, event_actor_pseudonym, page_ref, pseudonymized_event_principal,
    KnowledgeBlockRecord, KnowledgePageError, KnowledgePageRecord, KnowledgePageStore,
    KnowledgeVisibility, NewKnowledgePage, SaveKnowledgePage,
};
use myelin_storage::encryption::{EncryptedColumn, SubjectId};
use myelin_storage::encryption::KeyChoiceError;
use myelin_storage::kms::{KeyClass, KmsEngine, KmsError};
use myelin_tenancy::{Region, TenantId};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::{Handle, RuntimeFlavor};

const MAX_KNOWLEDGE_JSON_BYTES: usize = 320 * 1024;
const MAX_TITLE_BYTES: usize = 512;
const MAX_BLOCK_BYTES: usize = 64 * 1024;
const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const MAX_BLOCKS: usize = 500;
const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;

#[derive(Clone)]
pub struct DurableKnowledgeReadApi {
    store: KnowledgePageStore,
    runtime: Handle,
    kms: Arc<KmsEngine>,
}

impl DurableKnowledgeReadApi {
    pub fn new(pool: PgPool, runtime: Handle, kms: Arc<KmsEngine>) -> Self {
        Self {
            store: KnowledgePageStore::new(pool),
            runtime,
            kms,
        }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: Future<Output = Result<T, KnowledgePageError>>,
    {
        let result = match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.runtime.block_on(future))
            }
            Ok(_) => {
                return Err(EdgeError::Unavailable(
                    "Knowledge requires the Edge multi-thread runtime".into(),
                ))
            }
            Err(_) => self.runtime.block_on(future),
        };
        result.map_err(map_page_error)
    }

    fn viewer(&self, principal: &Principal) -> String {
        event_actor_pseudonym(&principal.tenant.0, &principal.principal_id.0)
    }

    pub fn list_pages(
        &self,
        principal: &Principal,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<Value, EdgeError> {
        validate_page_query(limit, cursor.as_deref())?;
        let viewer = self.viewer(principal);
        let pages = self.drive(self.store.list_visible(
            &principal.tenant.0,
            &principal.region.0,
            &viewer,
            cursor.as_deref(),
            limit + 1,
        ))?;
        let has_more = pages.len() > limit as usize;
        let visible = &pages[..pages.len().min(limit as usize)];
        let next = has_more
            .then(|| visible.last().map(|page| page.page_id.clone()))
            .flatten();
        let items = visible
            .iter()
            .map(|page| summary_json(page, &viewer, self.kms.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(page_envelope(json!(items), next, limit as usize))
    }

    pub fn read_page(&self, principal: &Principal, page_id: &str) -> Result<Value, EdgeError> {
        validate_ulid(page_id)?;
        let viewer = self.viewer(principal);
        let page = self.drive(self.store.get_visible(
            &principal.tenant.0,
            &principal.region.0,
            &viewer,
            page_id,
        ))?;
        document_json(&page, &viewer, self.kms.as_ref())
    }
}

#[derive(Clone)]
struct DurableKnowledgeApi {
    reads: DurableKnowledgeReadApi,
    ids: Arc<dyn IdMinter>,
}

impl DurableKnowledgeApi {
    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: Future<Output = Result<T, KnowledgePageError>>,
    {
        self.reads.drive(future)
    }

    fn mint_id(&self) -> String {
        self.ids.mint().0
    }

    fn store(&self) -> &KnowledgePageStore {
        &self.reads.store
    }

    fn kms(&self) -> &KmsEngine {
        self.reads.kms.as_ref()
    }

    fn viewer(&self, principal: &Principal) -> String {
        self.reads.viewer(principal)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePageBody {
    title: String,
    template: String,
    visibility: String,
    client_nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavePageBody {
    expected_version: i64,
    title: String,
    visibility: String,
    blocks: Vec<BlockBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockBody {
    id: Option<String>,
    #[serde(rename = "type")]
    block_type: String,
    markdown: String,
    #[serde(default = "active_block_state")]
    state: String,
}

fn active_block_state() -> String {
    "active".into()
}

struct PageListHandler {
    api: DurableKnowledgeReadApi,
}

impl Handler for PageListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let (limit, cursor) = parse_page_query(&ctx.request.query)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.list_pages(ctx.principal, limit, cursor)?,
        )))
    }
}

struct PageCreateHandler {
    api: DurableKnowledgeApi,
}

impl Handler for PageCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body: CreatePageBody = parse_body(&ctx.request.body)?;
        validate_title(&body.title)?;
        let client_nonce = client_nonce(
            ctx.request,
            &ctx.principal.principal_id.0,
            body.client_nonce.as_deref(),
        )?;
        let visibility = parse_visibility(&body.visibility)?;
        let template = template_blocks(&body.template)?;
        let page_id = self.api.mint_id();
        let viewer = self.api.viewer(ctx.principal);
        let title = seal(
            self.api.kms(),
            ctx.principal,
            &viewer,
            &title_scope(&page_id),
            body.title.as_bytes(),
        )?;
        let blocks = template
            .into_iter()
            .map(|(block_type, markdown)| {
                let block_id = self.api.mint_id();
                Ok(KnowledgeBlockRecord {
                    inline: seal(
                        self.api.kms(),
                        ctx.principal,
                        &viewer,
                        &block_scope(&page_id, &block_id),
                        markdown.as_bytes(),
                    )?,
                    block_id,
                    block_type: block_type.into(),
                    created_by: viewer.clone(),
                    edited_by: viewer.clone(),
                })
            })
            .collect::<Result<Vec<_>, EdgeError>>()?;
        let event_actor = pseudonymized_event_principal(&ctx.principal.tenant.0, ctx.principal);
        let page = NewKnowledgePage {
            tenant: ctx.principal.tenant.0.clone(),
            region: ctx.principal.region.0.clone(),
            page_id: page_id.clone(),
            space_key: "engineering".into(),
            parent_page_id: None,
            title,
            owner: viewer.clone(),
            visibility,
            client_nonce,
            blocks,
        };
        let (stored_id, created) = self.api.drive(self.api.store().create(
            &page,
            EventId(self.api.mint_id()),
            Actor(event_actor),
            now_timestamp(),
        ))?;
        let stored = self.api.reads.read_page(ctx.principal, &stored_id)?;
        Ok(no_store(EdgeResponse::json(
            if created { 201 } else { 200 },
            &json!({
                "page": stored,
                "created": created,
                "durable": true,
            }),
        )))
    }
}

struct PageGetHandler {
    api: DurableKnowledgeReadApi,
}

impl Handler for PageGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let page_id = page_param(ctx)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({ "page": self.api.read_page(ctx.principal, page_id)? }),
        )))
    }
}

struct PageSaveHandler {
    api: DurableKnowledgeApi,
}

impl Handler for PageSaveHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let page_id = page_param(ctx)?.to_string();
        let body: SavePageBody = parse_body(&ctx.request.body)?;
        validate_title(&body.title)?;
        validate_document(&body.blocks)?;
        let visibility = parse_visibility(&body.visibility)?;
        let viewer = self.api.viewer(ctx.principal);
        let current = self.api.drive(self.api.store().get_visible(
            &ctx.principal.tenant.0,
            &ctx.principal.region.0,
            &viewer,
            &page_id,
        ))?;
        if current.owner != viewer {
            return Err(EdgeError::NotFound("Knowledge page not found".into()));
        }

        let current_title = open_visible(
            self.api.kms(),
            &current,
            &current.title,
            &current.owner,
            &title_scope(&page_id),
        )?;
        let title = if current_title.as_deref() == Some(body.title.as_bytes()) {
            current.title.clone()
        } else {
            seal(
                self.api.kms(),
                ctx.principal,
                &viewer,
                &title_scope(&page_id),
                body.title.as_bytes(),
            )?
        };

        let current_blocks: HashMap<&str, &KnowledgeBlockRecord> = current
            .blocks
            .iter()
            .map(|block| (block.block_id.as_str(), block))
            .collect();
        let mut seen = HashSet::with_capacity(body.blocks.len());
        let mut blocks = Vec::with_capacity(body.blocks.len());
        for draft in body.blocks {
            let block_id = match draft.id {
                Some(id) => {
                    validate_ulid(&id)?;
                    id
                }
                None => self.api.mint_id(),
            };
            if !seen.insert(block_id.clone()) {
                return Err(EdgeError::BadRequest(
                    "Knowledge block ids must be unique".into(),
                ));
            }
            let existing = current_blocks.get(block_id.as_str()).copied();
            let visible_markdown = match existing {
                Some(block) if block.block_type == draft.block_type => open_visible(
                    self.api.kms(),
                    &current,
                    &block.inline,
                    &block.edited_by,
                    &block_scope(&page_id, &block_id),
                )?,
                _ => None,
            };
            let unchanged = if draft.state == "tombstoned" {
                existing.is_some_and(|block| block.block_type == draft.block_type)
                    && visible_markdown.is_none()
            } else {
                visible_markdown.as_deref() == Some(draft.markdown.as_bytes())
            };
            if unchanged {
                blocks.push(existing.expect("unchanged implies an existing block").clone());
            } else {
                blocks.push(KnowledgeBlockRecord {
                    inline: seal(
                        self.api.kms(),
                        ctx.principal,
                        &viewer,
                        &block_scope(&page_id, &block_id),
                        draft.markdown.as_bytes(),
                    )?,
                    block_id,
                    block_type: draft.block_type,
                    created_by: existing
                        .map(|block| block.created_by.clone())
                        .unwrap_or_else(|| viewer.clone()),
                    edited_by: viewer.clone(),
                });
            }
        }

        let event_actor = pseudonymized_event_principal(&ctx.principal.tenant.0, ctx.principal);
        let version = self.api.drive(self.api.store().save(
            &SaveKnowledgePage {
                tenant: ctx.principal.tenant.0.clone(),
                region: ctx.principal.region.0.clone(),
                page_id: page_id.clone(),
                owner: viewer.clone(),
                expected_version: body.expected_version,
                title,
                visibility,
                blocks,
            },
            EventId(self.api.mint_id()),
            Actor(event_actor),
            now_timestamp(),
        ))?;
        let saved = self.api.reads.read_page(ctx.principal, &page_id)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "page": saved,
                "version": version,
                "durable": true,
            }),
        )))
    }
}

pub fn register_knowledge(
    builder: GatewayBuilder,
    pool: PgPool,
    runtime: Handle,
    kms: Arc<KmsEngine>,
) -> GatewayBuilder {
    let reads = DurableKnowledgeReadApi::new(pool, runtime, kms);
    let api = DurableKnowledgeApi {
        reads: reads.clone(),
        ids: Arc::new(myelin_events::UlidMinter::new()),
    };
    builder
        .route(
            Method::Get,
            "/v1/knowledge/pages",
            "knowledge.pages.list",
            Arc::new(PageListHandler { api: reads.clone() }),
        )
        .route(
            Method::Post,
            "/v1/knowledge/pages",
            "knowledge.page.create",
            Arc::new(PageCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/knowledge/pages/{page}",
            "knowledge.page.view",
            Arc::new(PageGetHandler { api: reads }),
        )
        .route(
            Method::Put,
            "/v1/knowledge/pages/{page}",
            "knowledge.page.save",
            Arc::new(PageSaveHandler { api }),
        )
}

fn parse_body<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, EdgeError> {
    if body.is_empty() {
        return Err(EdgeError::BadRequest(
            "Knowledge request body is empty".into(),
        ));
    }
    if body.len() > MAX_KNOWLEDGE_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "Knowledge request exceeds the interactive document limit".into(),
        ));
    }
    serde_json::from_slice(body)
        .map_err(|error| EdgeError::BadRequest(format!("invalid Knowledge request: {error}")))
}

fn parse_page_query(query: &str) -> Result<(u32, Option<String>), EdgeError> {
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("malformed Knowledge query parameter".into())
            })?;
            match name {
                "limit" if limit.is_none() => {
                    limit = Some(value.parse::<u32>().map_err(|_| {
                        EdgeError::BadRequest("Knowledge limit must be an integer".into())
                    })?);
                }
                "cursor" if cursor.is_none() => {
                    cursor = Some(value.to_string());
                }
                "limit" | "cursor" => {
                    return Err(EdgeError::BadRequest(
                        "duplicate Knowledge query parameter".into(),
                    ))
                }
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown Knowledge query parameter `{other}`"
                    )))
                }
            }
        }
    }
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    validate_page_query(limit, cursor.as_deref())?;
    Ok((limit, cursor))
}

fn validate_page_query(limit: u32, cursor: Option<&str>) -> Result<(), EdgeError> {
    if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
        return Err(EdgeError::BadRequest(
            "Knowledge limit must be between 1 and 100".into(),
        ));
    }
    if let Some(cursor) = cursor {
        validate_ulid(cursor)?;
    }
    Ok(())
}

fn page_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let page = ctx
        .params
        .get("page")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a Knowledge page id".into()))?;
    validate_ulid(page)?;
    Ok(page)
}

fn validate_ulid(value: &str) -> Result<(), EdgeError> {
    if value.len() == 26
        && value
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
    {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "Knowledge ids and cursors must be canonical ULIDs".into(),
        ))
    }
}

fn validate_title(value: &str) -> Result<(), EdgeError> {
    if value.trim() == value
        && !value.is_empty()
        && value.len() <= MAX_TITLE_BYTES
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "Knowledge title must be 1-512 clean UTF-8 bytes without surrounding whitespace"
                .into(),
        ))
    }
}

fn validate_nonce(value: &str) -> Result<(), EdgeError> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "Knowledge client_nonce must be 1-128 URL-safe characters".into(),
        ))
    }
}

fn client_nonce(
    request: &crate::request::EdgeRequest,
    principal_id: &str,
    explicit: Option<&str>,
) -> Result<String, EdgeError> {
    match explicit {
        Some(value) => {
            validate_nonce(value)?;
            Ok(value.to_string())
        }
        None => request.stable_idempotency_nonce(principal_id),
    }
}

fn validate_document(blocks: &[BlockBody]) -> Result<(), EdgeError> {
    if blocks.is_empty() || blocks.len() > MAX_BLOCKS {
        return Err(EdgeError::BadRequest(
            "Knowledge pages must contain between 1 and 500 blocks".into(),
        ));
    }
    let mut total = 0usize;
    for block in blocks {
        if !matches!(block.state.as_str(), "active" | "tombstoned") {
            return Err(EdgeError::BadRequest(
                "Knowledge block state must be `active` or `tombstoned`".into(),
            ));
        }
        if block.state == "tombstoned" && (block.id.is_none() || !block.markdown.is_empty()) {
            return Err(EdgeError::BadRequest(
                "a tombstoned Knowledge block must retain its id and have no visible text".into(),
            ));
        }
        if block.markdown.len() > MAX_BLOCK_BYTES {
            return Err(EdgeError::PayloadTooLarge(
                "one Knowledge block exceeds 64 KiB".into(),
            ));
        }
        total = total.saturating_add(block.markdown.len());
        if total > MAX_DOCUMENT_BYTES {
            return Err(EdgeError::PayloadTooLarge(
                "Knowledge document exceeds 256 KiB".into(),
            ));
        }
        if block.markdown.contains('\0') {
            return Err(EdgeError::BadRequest(
                "Knowledge block contains an unsupported null character".into(),
            ));
        }
        if !matches!(
            block.block_type.as_str(),
            "paragraph"
                | "heading"
                | "bullet_list"
                | "ordered_list"
                | "task_list"
                | "blockquote"
                | "code_block"
                | "callout"
                | "divider"
        ) {
            return Err(EdgeError::BadRequest(
                "Knowledge block type is not in the shared content taxonomy".into(),
            ));
        }
    }
    Ok(())
}

fn parse_visibility(value: &str) -> Result<KnowledgeVisibility, EdgeError> {
    KnowledgeVisibility::parse(value).ok_or_else(|| {
        EdgeError::BadRequest("Knowledge visibility must be `private` or `team`".into())
    })
}

fn template_blocks(template: &str) -> Result<Vec<(&'static str, &'static str)>, EdgeError> {
    match template {
        "blank" => Ok(vec![("paragraph", "")]),
        "product-spec" => Ok(vec![
            ("heading", "Problem"),
            (
                "paragraph",
                "What user or organisational problem are we solving?",
            ),
            ("heading", "Outcomes"),
            (
                "bullet_list",
                "Describe the measurable change this work should create.",
            ),
            ("heading", "Delivery links"),
            (
                "paragraph",
                "Link the issues, pull requests, and decisions that carry this forward.",
            ),
        ]),
        "runbook" => Ok(vec![
            ("heading", "When to use this runbook"),
            (
                "paragraph",
                "Describe the symptoms and the service boundary.",
            ),
            ("heading", "Diagnosis"),
            (
                "ordered_list",
                "Record safe, reversible checks in the order they should run.",
            ),
            ("heading", "Recovery"),
            (
                "task_list",
                "Describe the recovery steps and how to verify the result.",
            ),
        ]),
        _ => Err(EdgeError::BadRequest(
            "Knowledge template must be `blank`, `product-spec`, or `runbook`".into(),
        )),
    }
}

fn seal(
    kms: &KmsEngine,
    principal: &Principal,
    subject: &str,
    scope: &str,
    plaintext: &[u8],
) -> Result<EncryptedColumn, EdgeError> {
    encrypt_text(
        kms,
        &principal.region,
        &principal.tenant,
        &SubjectId::new(subject),
        scope,
        plaintext,
    )
    .map_err(|error| EdgeError::Internal(format!("Knowledge encryption failed: {error}")))
}

fn open_visible(
    kms: &KmsEngine,
    page: &KnowledgePageRecord,
    column: &EncryptedColumn,
    subject: &str,
    scope: &str,
) -> Result<Option<Vec<u8>>, EdgeError> {
    if column.key_ref.tenant.as_str() != page.tenant
        || column.key_ref.class != KeyClass::Subject(subject.to_string())
    {
        return Err(EdgeError::Internal(
            "stored Knowledge encryption scope does not match its attribution".into(),
        ));
    }
    match decrypt_text(kms, &Region(page.region.clone()), column, scope) {
        Ok(plaintext) => Ok(Some(plaintext)),
        Err(KeyChoiceError::Kms(KmsError::DekUnavailable(_))) => Ok(None),
        Err(error) => Err(EdgeError::Internal(format!(
            "stored Knowledge text cannot be decrypted: {error}"
        ))),
    }
}

fn summary_json(
    page: &KnowledgePageRecord,
    viewer: &str,
    kms: &KmsEngine,
) -> Result<Value, EdgeError> {
    let title = open_visible(
        kms,
        page,
        &page.title,
        &page.owner,
        &title_scope(&page.page_id),
    )?
    .map(String::from_utf8)
    .transpose()
    .map_err(|_| EdgeError::Internal("stored Knowledge title is not valid UTF-8".into()))?;
    Ok(json!({
        "id": page.page_id,
        "ref": page_ref(&TenantId(page.tenant.clone()), &page.page_id).0,
        "space": page.space_key,
        "parent_page_id": page.parent_page_id,
        "title": title.as_deref().unwrap_or("[erased page title]"),
        "title_state": if title.is_some() { "active" } else { "tombstoned" },
        "visibility": page.visibility.as_str(),
        "version": page.version,
        "can_edit": page.owner == viewer,
        "created_at": page.created_at_epoch,
        "updated_at": page.updated_at_epoch,
    }))
}

fn document_json(
    page: &KnowledgePageRecord,
    viewer: &str,
    kms: &KmsEngine,
) -> Result<Value, EdgeError> {
    let blocks = page
        .blocks
        .iter()
        .map(|block| {
            let markdown = open_visible(
                kms,
                page,
                &block.inline,
                &block.edited_by,
                &block_scope(&page.page_id, &block.block_id),
            )?
            .map(String::from_utf8)
            .transpose()
            .map_err(|_| EdgeError::Internal("stored Knowledge block is not valid UTF-8".into()))?;
            Ok::<_, EdgeError>(json!({
                "id": block.block_id,
                "type": block.block_type,
                "markdown": markdown.as_deref().unwrap_or(""),
                "state": if markdown.is_some() { "active" } else { "tombstoned" },
                "is_you": block.edited_by == viewer,
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut page_json = summary_json(page, viewer, kms)?;
    page_json["blocks"] = json!(blocks);
    Ok(page_json)
}

fn title_scope(page_id: &str) -> String {
    format!("page:{page_id}:title")
}

fn block_scope(page_id: &str, block_id: &str) -> String {
    format!("page:{page_id}:block:{block_id}")
}

fn map_page_error(error: KnowledgePageError) -> EdgeError {
    match error {
        KnowledgePageError::NotFound => EdgeError::NotFound("Knowledge page not found".into()),
        KnowledgePageError::Conflict { current_version } => EdgeError::Conflict(format!(
            "Knowledge page changed while you were editing; current version is {current_version}"
        )),
        KnowledgePageError::Invalid(reason) => EdgeError::BadRequest(reason),
        KnowledgePageError::Storage(reason) => EdgeError::Internal(reason),
    }
}

fn require_empty_query(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.query.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "Knowledge page operation accepts no query parameters".into(),
        ))
    }
}

fn now_timestamp() -> Timestamp {
    let now = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
    Timestamp(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_document_inputs_are_strict_and_bounded() {
        assert_eq!(parse_page_query(""), Ok((50, None)));
        assert!(parse_page_query("limit=0").is_err());
        assert!(parse_page_query("cursor=nope").is_err());
        assert!(parse_page_query("offset=1").is_err());
        assert!(validate_title("Architecture decisions").is_ok());
        assert!(validate_title(" Architecture decisions").is_err());
        assert!(validate_document(&[BlockBody {
            id: None,
            block_type: "paragraph".into(),
            markdown: "One render path".into(),
            state: "active".into(),
        }])
        .is_ok());
        assert!(validate_document(&[]).is_err());
    }

    #[test]
    fn templates_teach_real_work_without_fake_activity() {
        assert_eq!(template_blocks("blank").expect("blank").len(), 1);
        let spec = template_blocks("product-spec").expect("spec");
        assert!(spec.iter().any(|(_, text)| text.contains("pull requests")));
        assert!(template_blocks("not-a-template").is_err());
    }
}
