use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue, OrderKey};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Schema, FAST, INDEXED, STORED, STRING, TEXT};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

#[derive(Clone, Debug, PartialEq)]
pub struct IndexDocument {
    pub doc_id: String,
    pub acl_object: String,
    pub text: String,
    pub fields: BTreeMap<String, FieldValue>,
    pub embedding: Option<crate::vector::Embedding>,
    pub model_ref: Option<crate::vector::ModelRef>,
    pub lang: Option<String>,
}

impl IndexDocument {
    pub fn new(doc_id: impl Into<String>, text: impl Into<String>) -> IndexDocument {
        let doc_id = doc_id.into();
        IndexDocument {
            acl_object: doc_id.clone(),
            doc_id,
            text: text.into(),
            fields: BTreeMap::new(),
            embedding: None,
            model_ref: None,
            lang: None,
        }
    }

    pub fn with_lang(mut self, lang: impl Into<String>) -> IndexDocument {
        self.lang = Some(lang.into());
        self
    }

    pub fn with_acl_object(mut self, acl_object: impl Into<String>) -> IndexDocument {
        self.acl_object = acl_object.into();
        self
    }

    pub fn with_field(mut self, name: impl Into<String>, value: FieldValue) -> IndexDocument {
        self.fields.insert(name.into(), value);
        self
    }

    pub fn with_embedding(
        mut self,
        embedding: crate::vector::Embedding,
        model_ref: impl Into<crate::vector::ModelRef>,
    ) -> IndexDocument {
        self.embedding = Some(embedding);
        self.model_ref = Some(model_ref.into());
        self
    }

    pub fn order_key(&self) -> Option<&OrderKey> {
        match self.fields.get(ORDER_KEY_FIELD) {
            Some(FieldValue::OrderKey(k)) => Some(k),
            _ => None,
        }
    }
}

pub const ORDER_KEY_FIELD: &str = "order_key";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AclFilter {
    All,
    None,
    Ids(Vec<String>),
    NotIds(Vec<String>),
    And(Vec<AclFilter>),
    Or(Vec<AclFilter>),
    Not(Box<AclFilter>),
}

impl AclFilter {
    pub fn ids(ids: impl IntoIterator<Item = impl Into<String>>) -> AclFilter {
        AclFilter::Ids(ids.into_iter().map(Into::into).collect())
    }

    pub fn not_ids(ids: impl IntoIterator<Item = impl Into<String>>) -> AclFilter {
        AclFilter::NotIds(ids.into_iter().map(Into::into).collect())
    }

    pub fn admits(&self, doc_id: &str, acl_object: &str) -> bool {
        match self {
            AclFilter::All => true,
            AclFilter::None => false,
            AclFilter::Ids(ids) => ids.iter().any(|i| i == doc_id || i == acl_object),
            AclFilter::NotIds(ids) => !ids.iter().any(|i| i == doc_id || i == acl_object),
            AclFilter::And(subs) => subs.iter().all(|s| s.admits(doc_id, acl_object)),
            AclFilter::Or(subs) => {
                !subs.is_empty() && subs.iter().any(|s| s.admits(doc_id, acl_object))
            }
            AclFilter::Not(inner) => !inner.admits(doc_id, acl_object),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub doc_id: String,
    pub score: f32,
}

#[derive(Debug)]
pub enum IndexError {
    Engine(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::Engine(e) => write!(f, "index engine error: {e}"),
        }
    }
}

impl std::error::Error for IndexError {}

impl From<tantivy::TantivyError> for IndexError {
    fn from(e: tantivy::TantivyError) -> Self {
        IndexError::Engine(e.to_string())
    }
}

pub trait IndexBackend {
    fn upsert(&mut self, doc: &IndexDocument) -> Result<(), IndexError>;

    fn delete(&mut self, doc_id: &str) -> Result<(), IndexError>;

    fn search(
        &self,
        acl_filter: &AclFilter,
        text_query: &str,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError>;

    fn search_structured(
        &self,
        acl_filter: &AclFilter,
        field: &str,
        value: &FieldValue,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError>;

    fn semantic(
        &self,
        acl_filter: &AclFilter,
        query: &crate::vector::Embedding,
        k: usize,
    ) -> Result<Vec<crate::vector::VectorHit>, IndexError>;

    fn merge(&mut self) -> Result<(), IndexError>;

    fn snapshot(&mut self) -> Result<u64, IndexError>;

    fn indexed_zookie_of(&self, doc_id: &str) -> Option<String>;
}

pub struct TantivyBackend {
    index: Index,
    writer: IndexWriter,
    schema: SearchSchema,
    vectors: crate::vector::HnswVectorIndex,
    doc_meta: BTreeMap<String, DocMeta>,
}

#[derive(Clone)]
struct DocMeta {
    doc: IndexDocument,
    indexed_zookie: String,
    version: u64,
}

struct SearchSchema {
    doc_id: Field,
    acl_object: Field,
    text: Field,
    facets: BTreeMap<String, (Field, FieldType)>,
    order_key: Field,
    indexed_zookie: Field,
    version: Field,
    lang: Field,
}

enum AclQuery {
    Empty,
    Clause(Box<dyn Query>),
}

impl TantivyBackend {
    pub fn open(facets: &BTreeMap<String, FieldType>) -> Result<TantivyBackend, IndexError> {
        let mut builder = Schema::builder();
        let doc_id = builder.add_text_field("doc_id", STRING | STORED | FAST);
        let acl_object = builder.add_text_field("acl_object", STRING | FAST);
        let text = builder.add_text_field("text", TEXT | STORED);
        let order_key = builder.add_text_field(ORDER_KEY_FIELD, STRING | FAST);
        let indexed_zookie = builder.add_text_field("indexed_zookie", STRING | STORED | FAST);
        let version = builder.add_u64_field("version", INDEXED | STORED | FAST);
        let lang = builder.add_text_field("lang", STRING | STORED | FAST);

        let mut facet_fields = BTreeMap::new();
        for (name, ty) in facets {
            if name == ORDER_KEY_FIELD {
                facet_fields.insert(name.clone(), (order_key, *ty));
                continue;
            }
            let field = match ty {
                FieldType::Int => builder.add_i64_field(name, INDEXED | FAST | STORED),
                FieldType::Bool => builder.add_bool_field(name, INDEXED | FAST | STORED),
                FieldType::Text
                | FieldType::Date
                | FieldType::Select
                | FieldType::Relation
                | FieldType::Principal
                | FieldType::OrderKey => builder.add_text_field(name, STRING | FAST | STORED),
            };
            facet_fields.insert(name.clone(), (field, *ty));
        }

        let schema = builder.build();
        let index = Index::create_in_ram(schema);
        let writer = index.writer(15_000_000)?;
        Ok(TantivyBackend {
            index,
            writer,
            schema: SearchSchema {
                doc_id,
                acl_object,
                text,
                facets: facet_fields,
                order_key,
                indexed_zookie,
                version,
                lang,
            },
            vectors: crate::vector::HnswVectorIndex::open(),
            doc_meta: BTreeMap::new(),
        })
    }

    pub fn vectors(&self) -> &crate::vector::HnswVectorIndex {
        &self.vectors
    }

    fn add_facet(
        &self,
        doc: &mut TantivyDocument,
        field: Field,
        declared: FieldType,
        value: &FieldValue,
    ) -> Result<(), IndexError> {
        if value.field_type() != declared {
            return Err(IndexError::Engine(format!(
                "facet value of type {} does not match its declared FieldType {}",
                value.field_type().wire_id(),
                declared.wire_id()
            )));
        }
        match value {
            FieldValue::Int(n) => doc.add_i64(field, *n),
            FieldValue::Bool(b) => doc.add_bool(field, *b),
            FieldValue::Text(s)
            | FieldValue::Date(s)
            | FieldValue::Select(s)
            | FieldValue::Relation(s)
            | FieldValue::Principal(s) => doc.add_text(field, s),
            FieldValue::OrderKey(k) => doc.add_text(field, k.as_str()),
        }
        Ok(())
    }

    fn acl_clause(&self, ids: &[String]) -> Option<Box<dyn Query>> {
        if ids.is_empty() {
            return None;
        }
        let mut subs: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for id in ids {
            for field in [self.schema.doc_id, self.schema.acl_object] {
                let term = Term::from_field_text(field, id);
                subs.push((
                    Occur::Should,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                ));
            }
        }
        Some(Box::new(BooleanQuery::new(subs)))
    }

    fn deny_clause(&self, ids: &[String]) -> Option<Box<dyn Query>> {
        self.acl_clause(ids)
    }

    fn acl_query(&self, acl_filter: &AclFilter) -> AclQuery {
        match acl_filter {
            AclFilter::None => AclQuery::Empty,
            AclFilter::All => AclQuery::Clause(Box::new(tantivy::query::AllQuery)),
            AclFilter::Ids(ids) => match self.acl_clause(ids) {
                Some(q) => AclQuery::Clause(q),
                None => AclQuery::Empty,
            },
            AclFilter::NotIds(ids) => match self.deny_clause(ids) {
                Some(deny) => AclQuery::Clause(Box::new(BooleanQuery::new(vec![
                    (
                        Occur::Must,
                        Box::new(tantivy::query::AllQuery) as Box<dyn Query>,
                    ),
                    (Occur::MustNot, deny),
                ]))),
                None => AclQuery::Clause(Box::new(tantivy::query::AllQuery)),
            },
            AclFilter::And(subs) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
                for sub in subs {
                    match self.acl_query(sub) {
                        AclQuery::Empty => return AclQuery::Empty,
                        AclQuery::Clause(q) => clauses.push((Occur::Must, q)),
                    }
                }
                if clauses.is_empty() {
                    return AclQuery::Clause(Box::new(tantivy::query::AllQuery));
                }
                AclQuery::Clause(Box::new(BooleanQuery::new(clauses)))
            }
            AclFilter::Or(subs) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
                for sub in subs {
                    match self.acl_query(sub) {
                        AclQuery::Empty => {}
                        AclQuery::Clause(q) => clauses.push((Occur::Should, q)),
                    }
                }
                if clauses.is_empty() {
                    return AclQuery::Empty;
                }
                AclQuery::Clause(Box::new(BooleanQuery::new(clauses)))
            }
            AclFilter::Not(inner) => match self.acl_query(inner) {
                AclQuery::Empty => AclQuery::Clause(Box::new(tantivy::query::AllQuery)),
                AclQuery::Clause(q) => AclQuery::Clause(Box::new(BooleanQuery::new(vec![
                    (
                        Occur::Must,
                        Box::new(tantivy::query::AllQuery) as Box<dyn Query>,
                    ),
                    (Occur::MustNot, q),
                ]))),
            },
        }
    }

    pub fn segment_count(&self) -> Result<usize, IndexError> {
        Ok(self.index.searchable_segment_ids()?.len())
    }

    fn doc_id_of(
        &self,
        searcher: &tantivy::Searcher,
        addr: tantivy::DocAddress,
    ) -> Result<String, IndexError> {
        use tantivy::schema::Value;
        let doc: TantivyDocument = searcher.doc(addr)?;
        let v = doc
            .get_first(self.schema.doc_id)
            .and_then(|v| v.as_str())
            .ok_or_else(|| IndexError::Engine("a hit has no stored doc_id".into()))?;
        Ok(v.to_string())
    }
}

impl TantivyBackend {
    pub fn upsert_stamped(
        &mut self,
        doc: &IndexDocument,
        indexed_zookie: &str,
        version: u64,
    ) -> Result<(), IndexError> {
        let key = Term::from_field_text(self.schema.doc_id, &doc.doc_id);
        self.writer.delete_term(key);

        let mut td = TantivyDocument::default();
        td.add_text(self.schema.doc_id, &doc.doc_id);
        td.add_text(self.schema.acl_object, &doc.acl_object);
        td.add_text(self.schema.text, &doc.text);
        td.add_text(self.schema.indexed_zookie, indexed_zookie);
        td.add_u64(self.schema.version, version);
        if let Some(lang) = &doc.lang {
            td.add_text(self.schema.lang, lang);
        }

        for (name, value) in &doc.fields {
            if name == ORDER_KEY_FIELD {
                if let FieldValue::OrderKey(k) = value {
                    td.add_text(self.schema.order_key, k.as_str());
                    continue;
                }
                return Err(IndexError::Engine(
                    "the `order_key` facet must be a FieldValue::OrderKey".into(),
                ));
            }
            let (field, declared) = self.schema.facets.get(name).copied().ok_or_else(|| {
                IndexError::Engine(format!("facet `{name}` was not declared at open()"))
            })?;
            self.add_facet(&mut td, field, declared, value)?;
        }

        self.writer.add_document(td)?;
        self.writer.commit()?;

        match (&doc.embedding, &doc.model_ref) {
            (Some(embedding), Some(model_ref)) => {
                self.vectors.upsert(crate::vector::VectorRecord {
                    doc_id: doc.doc_id.clone(),
                    acl_object: doc.acl_object.clone(),
                    embedding: embedding.clone(),
                    model_ref: model_ref.clone(),
                })?;
            }
            (Some(_), None) => {
                return Err(IndexError::Engine(
                    "an embedding requires a model_ref (a vector must pin its model - §3.3)".into(),
                ));
            }
            (None, _) => {
                self.vectors.soft_delete(&doc.doc_id);
            }
        }

        self.doc_meta.insert(
            doc.doc_id.clone(),
            DocMeta {
                doc: doc.clone(),
                indexed_zookie: indexed_zookie.to_string(),
                version,
            },
        );
        Ok(())
    }

    pub fn indexed_zookie_of(&self, doc_id: &str) -> Option<String> {
        self.doc_meta.get(doc_id).map(|m| m.indexed_zookie.clone())
    }

    pub fn restamp_zookie(&mut self, doc_id: &str, new_zookie: &str) {
        let Some(meta) = self.doc_meta.get(doc_id).cloned() else {
            return;
        };
        let _ = self.upsert_stamped(&meta.doc, new_zookie, meta.version + 1);
    }

    pub fn locate_subject(&self, matcher: &SubjectMatcher) -> Vec<String> {
        let mut out: Vec<String> = self
            .doc_meta
            .iter()
            .filter(|(_, meta)| matcher.matches(&meta.doc))
            .map(|(doc_id, _)| doc_id.clone())
            .collect();
        out.sort_unstable();
        out
    }
}

#[derive(Clone, Debug)]
pub struct SubjectMatcher {
    subject_id: String,
    pseudonym: Option<String>,
    locator_facets: Vec<String>,
}

pub const DEFAULT_SUBJECT_LOCATOR_FACETS: &[&str] = &["actor", "assignee", "mention"];

impl SubjectMatcher {
    pub fn new(subject_id: impl Into<String>, pseudonym: Option<String>) -> SubjectMatcher {
        SubjectMatcher {
            subject_id: subject_id.into(),
            pseudonym,
            locator_facets: DEFAULT_SUBJECT_LOCATOR_FACETS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    pub fn with_locator_facets(
        mut self,
        facets: impl IntoIterator<Item = impl Into<String>>,
    ) -> SubjectMatcher {
        self.locator_facets = facets.into_iter().map(Into::into).collect();
        self
    }

    pub fn subject_id(&self) -> &str {
        &self.subject_id
    }

    pub fn matches(&self, doc: &IndexDocument) -> bool {
        if doc.acl_object == self.subject_id || doc.doc_id == self.subject_id {
            return true;
        }
        for facet in &self.locator_facets {
            if let Some(value) = doc.fields.get(facet) {
                if Self::facet_text(value).as_deref() == Some(self.subject_id.as_str()) {
                    return true;
                }
            }
        }
        if let Some(pseudonym) = &self.pseudonym {
            if doc.text.contains(pseudonym.as_str()) {
                return true;
            }
        }
        false
    }

    fn facet_text(value: &FieldValue) -> Option<String> {
        match value {
            FieldValue::Text(s)
            | FieldValue::Date(s)
            | FieldValue::Select(s)
            | FieldValue::Relation(s)
            | FieldValue::Principal(s) => Some(s.clone()),
            FieldValue::OrderKey(k) => Some(k.as_str().to_string()),
            FieldValue::Int(_) | FieldValue::Bool(_) => None,
        }
    }
}

impl IndexBackend for TantivyBackend {
    fn upsert(&mut self, doc: &IndexDocument) -> Result<(), IndexError> {
        self.upsert_stamped(doc, "", 0)
    }

    fn delete(&mut self, doc_id: &str) -> Result<(), IndexError> {
        let key = Term::from_field_text(self.schema.doc_id, doc_id);
        self.writer.delete_term(key);
        self.writer.commit()?;
        self.vectors.soft_delete(doc_id);
        self.doc_meta.remove(doc_id);
        Ok(())
    }

    fn search(
        &self,
        acl_filter: &AclFilter,
        text_query: &str,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError> {
        let acl_clause: Box<dyn Query> = match self.acl_query(acl_filter) {
            AclQuery::Empty => return Ok(Vec::new()),
            AclQuery::Clause(q) => q,
        };

        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(&self.index, vec![self.schema.text]);
        let ft: Box<dyn Query> = parser
            .parse_query(text_query)
            .map_err(|e| IndexError::Engine(format!("query parse: {e}")))?;

        let acl_filtered_plan =
            BooleanQuery::new(vec![(Occur::Must, acl_clause), (Occur::Must, ft)]);
        let top = searcher.search(
            &acl_filtered_plan,
            &TopDocs::with_limit(limit.max(1)).order_by_score(),
        )?;

        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            hits.push(Hit {
                doc_id: self.doc_id_of(&searcher, addr)?,
                score,
            });
        }
        Ok(hits)
    }

    fn search_structured(
        &self,
        acl_filter: &AclFilter,
        field: &str,
        value: &FieldValue,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError> {
        let acl_clause: Box<dyn Query> = match self.acl_query(acl_filter) {
            AclQuery::Empty => return Ok(Vec::new()),
            AclQuery::Clause(q) => q,
        };

        let (tf, declared) = self.schema.facets.get(field).copied().ok_or_else(|| {
            IndexError::Engine(format!(
                "structured facet `{field}` was not declared at open()"
            ))
        })?;
        if value.field_type() != declared {
            return Err(IndexError::Engine(format!(
                "structured predicate on `{field}` has type {} but the facet is {}",
                value.field_type().wire_id(),
                declared.wire_id()
            )));
        }

        let facet_term = match value {
            FieldValue::Int(n) => Term::from_field_i64(tf, *n),
            FieldValue::Bool(b) => Term::from_field_bool(tf, *b),
            FieldValue::Text(s)
            | FieldValue::Date(s)
            | FieldValue::Select(s)
            | FieldValue::Relation(s)
            | FieldValue::Principal(s) => Term::from_field_text(tf, s),
            FieldValue::OrderKey(k) => Term::from_field_text(tf, k.as_str()),
        };
        let facet_q: Box<dyn Query> =
            Box::new(TermQuery::new(facet_term, IndexRecordOption::Basic));

        let acl_filtered_plan =
            BooleanQuery::new(vec![(Occur::Must, acl_clause), (Occur::Must, facet_q)]);

        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let collector = TopDocs::with_limit(limit.max(1))
            .order_by_string_fast_field(ORDER_KEY_FIELD, tantivy::Order::Asc);
        let top = searcher.search(&acl_filtered_plan, &collector)?;
        let mut hits = Vec::with_capacity(top.len());
        for (_order_key, addr) in top {
            hits.push(Hit {
                doc_id: self.doc_id_of(&searcher, addr)?,
                score: 0.0,
            });
        }
        Ok(hits)
    }

    fn semantic(
        &self,
        acl_filter: &AclFilter,
        query: &crate::vector::Embedding,
        k: usize,
    ) -> Result<Vec<crate::vector::VectorHit>, IndexError> {
        let hits = match acl_filter {
            AclFilter::None => Vec::new(),
            AclFilter::All => self.vectors.knn(query, k),
            _ => self.vectors.knn_filtered(query, k, |doc_id, acl_object| {
                acl_filter.admits(doc_id, acl_object)
            }),
        };
        Ok(hits)
    }

    fn merge(&mut self) -> Result<(), IndexError> {
        self.vectors.compact();
        self.writer.commit()?;
        let segment_ids = self.index.searchable_segment_ids()?;
        if segment_ids.len() > 1 {
            self.writer.merge(&segment_ids).wait()?;
        }
        Ok(())
    }

    fn snapshot(&mut self) -> Result<u64, IndexError> {
        self.writer.commit()?;
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        Ok(searcher.num_docs())
    }

    fn indexed_zookie_of(&self, doc_id: &str) -> Option<String> {
        TantivyBackend::indexed_zookie_of(self, doc_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facet_decl() -> BTreeMap<String, FieldType> {
        let mut m = BTreeMap::new();
        m.insert("status".to_string(), FieldType::Select);
        m.insert("severity".to_string(), FieldType::Int);
        m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
        m
    }

    fn doc(id: &str, text: &str, status: &str, severity: i64, ord: &OrderKey) -> IndexDocument {
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select(status.into()))
            .with_field("severity", FieldValue::Int(severity))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(ord.clone()))
    }

    #[test]
    fn full_text_shape_round_trips() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc(
            "acme/issue/ENG-1",
            "deadlock in the scheduler",
            "open",
            3,
            &k,
        ))
        .expect("upsert");
        be.upsert(&doc(
            "acme/issue/ENG-2",
            "typo in the readme",
            "open",
            1,
            &k,
        ))
        .expect("upsert");

        let acl_filter = AclFilter::ids(["acme/issue/ENG-1", "acme/issue/ENG-2"]);
        let hits = be.search(&acl_filter, "deadlock", 10).expect("search");
        assert_eq!(hits.len(), 1, "one doc mentions `deadlock`");
        assert_eq!(hits[0].doc_id, "acme/issue/ENG-1");

        be.delete("acme/issue/ENG-1").expect("delete");
        let hits = be
            .search(&acl_filter, "deadlock", 10)
            .expect("search after delete");
        assert!(hits.is_empty(), "the deleted doc no longer surfaces");
    }

    #[test]
    fn structured_shape_round_trips() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("d1", "alpha", "open", 5, &k))
            .expect("upsert");
        be.upsert(&doc("d2", "beta", "closed", 5, &k))
            .expect("upsert");
        be.upsert(&doc("d3", "gamma", "open", 2, &k))
            .expect("upsert");

        let acl_filter = AclFilter::ids(["d1", "d2", "d3"]);
        let open = be
            .search_structured(
                &acl_filter,
                "status",
                &FieldValue::Select("open".into()),
                10,
            )
            .expect("structured search");
        let ids: std::collections::BTreeSet<String> = open.into_iter().map(|h| h.doc_id).collect();
        assert_eq!(
            ids,
            ["d1", "d3"].iter().map(|s| s.to_string()).collect(),
            "only the `status == open` docs match"
        );

        let sev5 = be
            .search_structured(&acl_filter, "severity", &FieldValue::Int(5), 10)
            .expect("int facet search");
        let ids: std::collections::BTreeSet<String> = sev5.into_iter().map(|h| h.doc_id).collect();
        assert_eq!(ids, ["d1", "d2"].iter().map(|s| s.to_string()).collect());
    }

    #[test]
    fn acl_filter_pre_filters_before_scoring() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("secret", "deadlock secret", "open", 9, &k))
            .expect("upsert");
        be.upsert(&doc("visible", "deadlock visible", "open", 9, &k))
            .expect("upsert");

        assert!(be
            .search(&AclFilter::None, "deadlock", 10)
            .expect("none")
            .is_empty());

        let acl_filter = AclFilter::ids(["visible"]);
        let hits = be.search(&acl_filter, "deadlock", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "visible");
    }

    #[test]
    fn acl_all_admits_every_matching_doc() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("a", "deadlock one", "open", 1, &k))
            .expect("upsert");
        be.upsert(&doc("b", "deadlock two", "open", 1, &k))
            .expect("upsert");
        let hits = be
            .search(&AclFilter::All, "deadlock", 10)
            .expect("admin search");
        assert_eq!(hits.len(), 2, "admin sees both matching docs");
    }

    #[test]
    fn upsert_is_idempotent_on_doc_id() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("d", "first text", "open", 1, &k))
            .expect("upsert");
        be.upsert(&doc("d", "second text", "open", 1, &k))
            .expect("re-upsert");
        let acl_filter = AclFilter::ids(["d"]);
        assert!(
            be.search(&acl_filter, "first", 10).expect("s1").is_empty(),
            "old text replaced"
        );
        assert_eq!(
            be.search(&acl_filter, "second", 10).expect("s2").len(),
            1,
            "new text indexed"
        );
        assert_eq!(
            be.snapshot().expect("snapshot"),
            1,
            "exactly one live doc (no dupe)"
        );
    }

    #[test]
    fn structured_shape_is_typed_over_the_frozen_field_type() {
        let mut decl = BTreeMap::new();
        for (i, ty) in FieldType::all().into_iter().enumerate() {
            decl.insert(format!("f{i}_{}", ty.wire_id()), ty);
        }
        let be = TantivyBackend::open(&decl).expect("open over the full frozen FieldType taxonomy");
        for (i, ty) in FieldType::all().into_iter().enumerate() {
            let name = format!("f{i}_{}", ty.wire_id());
            let (_, declared) = be
                .schema
                .facets
                .get(&name)
                .copied()
                .expect("facet declared");
            assert_eq!(
                declared,
                ty,
                "facet `{name}` is typed over FieldType::{}",
                ty.wire_id()
            );
        }

        let wire_ids: Vec<&str> = FieldType::all().iter().map(|t| t.wire_id()).collect();
        assert_eq!(
            wire_ids,
            ["text", "int", "bool", "date", "select", "relation", "principal", "order_key"],
            "the frozen FieldType taxonomy the Search structured shape is typed over (byte-identical \
             to Issues'/Knowledge's encoding) - a rename breaks Search now"
        );

        let mut decl2 = BTreeMap::new();
        decl2.insert("severity".to_string(), FieldType::Int);
        let mut be2 = TantivyBackend::open(&decl2).expect("open");
        let bad = IndexDocument::new("d", "x")
            .with_field("severity", FieldValue::Text("not-an-int".into()));
        let err = be2
            .upsert(&bad)
            .expect_err("a type mismatch must be rejected");
        assert!(
            matches!(err, IndexError::Engine(_)),
            "loud rejection, not a silent coerce"
        );
    }

    #[test]
    fn order_key_columnar_fast_field_sorts() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k1 = OrderKey::parse("G").unwrap();
        let k3 = OrderKey::parse("V").unwrap();
        let k2 = OrderKey::bisect(Some(&k1), Some(&k3));
        assert!(k1 < k2 && k2 < k3, "the keys are strictly ordered");

        be.upsert(&doc("mid", "x", "open", 1, &k2)).expect("upsert");
        be.upsert(&doc("first", "x", "open", 1, &k1))
            .expect("upsert");
        be.upsert(&doc("last", "x", "open", 1, &k3))
            .expect("upsert");

        let acl_filter = AclFilter::ids(["first", "mid", "last"]);
        for (id, key) in [("first", &k1), ("mid", &k2), ("last", &k3)] {
            let hits = be
                .search_structured(
                    &acl_filter,
                    ORDER_KEY_FIELD,
                    &FieldValue::OrderKey(key.clone()),
                    10,
                )
                .expect("order_key facet search");
            let ids: Vec<String> = hits.into_iter().map(|h| h.doc_id).collect();
            assert_eq!(
                ids,
                vec![id.to_string()],
                "the order_key fast-field keys `{id}` uniquely"
            );
        }
    }

    #[test]
    fn merge_and_snapshot_operate() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        for i in 0..5 {
            be.upsert(&doc(&format!("d{i}"), "body", "open", i, &k))
                .expect("upsert");
        }
        assert_eq!(be.snapshot().expect("snapshot"), 5, "five live docs");
        assert!(
            be.segment_count().expect("segments") > 1,
            "multiple segments accumulated"
        );

        be.delete("d2").expect("delete");
        assert_eq!(
            be.snapshot().expect("snapshot"),
            4,
            "one fewer after delete"
        );

        be.merge().expect("merge compacts the segments");
        assert_eq!(
            be.snapshot().expect("snapshot after merge"),
            4,
            "merge preserves the live set"
        );
        assert_eq!(
            be.segment_count().expect("segments after merge"),
            1,
            "merge compacts the multiple segments to ONE (the `>1` guard fired and actually merged)"
        );
    }

    #[test]
    fn merge_is_a_noop_with_one_segment() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("only", "body", "open", 1, &k))
            .expect("upsert");
        assert_eq!(be.segment_count().expect("segments"), 1, "one segment");
        be.merge().expect("merge");
        assert_eq!(
            be.segment_count().expect("segments"),
            1,
            "still one segment (no-op)"
        );
        assert_eq!(
            be.snapshot().expect("snapshot"),
            1,
            "the live doc is intact"
        );
    }

    #[test]
    fn index_document_exposes_its_order_key() {
        let k = OrderKey::parse("V5").unwrap();
        let with = IndexDocument::new("d", "x")
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()));
        assert_eq!(
            with.order_key(),
            Some(&k),
            "the order_key fast-field is exposed"
        );

        let without =
            IndexDocument::new("d", "x").with_field("status", FieldValue::Select("open".into()));
        assert_eq!(without.order_key(), None, "no order_key ⇒ None");

        let wrong = IndexDocument::new("d", "x")
            .with_field(ORDER_KEY_FIELD, FieldValue::Text("not-a-key".into()));
        assert_eq!(
            wrong.order_key(),
            None,
            "a wrongly-typed order_key facet is None"
        );
    }

    #[test]
    fn vector_shape_round_trips_through_the_trait() {
        use crate::vector::Embedding;
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        let embed = |id: &str, v: Vec<f32>| {
            doc(id, "body", "open", 1, &k).with_embedding(Embedding::new(v), "text-embed@1")
        };
        be.upsert(&embed("acme/doc/A", vec![1.0, 0.0, 0.0]))
            .expect("upsert A");
        be.upsert(&embed("acme/doc/B", vec![0.0, 1.0, 0.0]))
            .expect("upsert B");
        be.upsert(&embed("acme/doc/C", vec![0.9, 0.1, 0.0]))
            .expect("upsert C");

        let acl_filter = AclFilter::ids(["acme/doc/A", "acme/doc/B", "acme/doc/C"]);
        let hits = be
            .semantic(&acl_filter, &Embedding::new(vec![1.0, 0.05, 0.0]), 2)
            .expect("semantic");
        assert_eq!(hits.len(), 2);
        let ids: std::collections::BTreeSet<&str> =
            hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids.contains("acme/doc/A") && ids.contains("acme/doc/C"),
            "A and C nearest: {ids:?}"
        );
        assert!(hits
            .iter()
            .all(|h| h.model_ref == crate::vector::ModelRef("text-embed@1".into())));

        be.delete("acme/doc/B").expect("delete B");
        assert!(
            be.vectors().has_orphan_embedding(),
            "B's vector tombstoned but physically present"
        );
        let bhit = be
            .semantic(&acl_filter, &Embedding::new(vec![0.0, 1.0, 0.0]), 3)
            .expect("semantic");
        assert!(
            !bhit.iter().any(|h| h.doc_id == "acme/doc/B"),
            "the deleted vector never surfaces"
        );

        be.merge().expect("merge compacts vectors");
        assert!(
            !be.vectors().has_orphan_embedding(),
            "0 orphan embedding after merge (the GATE)"
        );
        assert_eq!(be.vectors().live_len(), 2, "A and C survive");
    }

    #[test]
    fn semantic_acl_pre_filters_no_leak() {
        use crate::vector::Embedding;
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        let embed = |id: &str, v: Vec<f32>| {
            doc(id, "body", "open", 1, &k).with_embedding(Embedding::new(v), "m@1")
        };
        be.upsert(&embed("secret", vec![1.0, 0.0])).expect("u");
        be.upsert(&embed("visible", vec![0.8, 0.2])).expect("u");

        assert!(be
            .semantic(&AclFilter::None, &Embedding::new(vec![1.0, 0.0]), 5)
            .unwrap()
            .is_empty());

        let acl_filter = AclFilter::ids(["visible"]);
        let hits = be
            .semantic(&acl_filter, &Embedding::new(vec![1.0, 0.0]), 5)
            .expect("semantic");
        let ids: Vec<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["visible"],
            "only the visible vector; the secret never surfaces"
        );
    }

    #[test]
    fn hybrid_query_fuses_on_one_doc_id_space() {
        use crate::vector::Embedding;
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(
            &doc(
                "acme/page/42",
                "distributed consensus and raft",
                "open",
                5,
                &k,
            )
            .with_embedding(Embedding::new(vec![1.0, 0.0, 0.0]), "m@1"),
        )
        .expect("upsert the tri-shape doc");
        be.upsert(
            &doc("acme/page/99", "frontend css layout", "closed", 2, &k)
                .with_embedding(Embedding::new(vec![0.0, 1.0, 0.0]), "m@1"),
        )
        .expect("upsert the other doc");

        let acl_filter = AclFilter::ids(["acme/page/42", "acme/page/99"]);

        let ft = be.search(&acl_filter, "raft", 10).expect("ft");
        assert_eq!(
            ft.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(),
            vec!["acme/page/42"]
        );

        let st = be
            .search_structured(
                &acl_filter,
                "status",
                &FieldValue::Select("open".into()),
                10,
            )
            .expect("structured");
        assert_eq!(
            st.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(),
            vec!["acme/page/42"]
        );

        let ve = be
            .semantic(&acl_filter, &Embedding::new(vec![0.95, 0.05, 0.0]), 1)
            .expect("semantic");
        assert_eq!(
            ve.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(),
            vec!["acme/page/42"]
        );

        assert_eq!(
            ft[0].doc_id, ve[0].doc_id,
            "keyword and vector hits share one doc_id"
        );
        assert_eq!(
            ft[0].doc_id, st[0].doc_id,
            "and the structured hit too - one doc-id space (§3.2)"
        );
    }

    #[test]
    fn reindex_dropping_embedding_removes_the_vector() {
        use crate::vector::Embedding;
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(
            &doc("d", "body", "open", 1, &k).with_embedding(Embedding::new(vec![1.0, 0.0]), "m@1"),
        )
        .expect("upsert with vector");
        let acl_filter = AclFilter::ids(["d"]);
        assert_eq!(
            be.semantic(&acl_filter, &Embedding::new(vec![1.0, 0.0]), 1)
                .unwrap()
                .len(),
            1
        );

        be.upsert(&doc("d", "body", "open", 1, &k))
            .expect("re-upsert without vector");
        assert!(
            be.semantic(&acl_filter, &Embedding::new(vec![1.0, 0.0]), 1)
                .unwrap()
                .is_empty(),
            "the dropped embedding leaves no orphan vector"
        );
    }

    #[test]
    fn embedding_without_model_ref_is_rejected() {
        use crate::vector::Embedding;
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let mut d = IndexDocument::new("d", "body");
        d.embedding = Some(Embedding::new(vec![1.0, 0.0]));
        d.model_ref = None;
        let err = be.upsert(&d).expect_err("must reject");
        assert!(
            matches!(err, IndexError::Engine(_)),
            "loud rejection: a vector needs a model_ref"
        );
    }

    #[test]
    fn boolean_composition_acl_filter_pre_filters() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        for id in ["a", "b", "c"] {
            be.upsert(&doc(id, "shared deadlock note", "open", 1, &k))
                .expect("upsert");
        }

        let acl_filter = AclFilter::Or(vec![AclFilter::ids(["a"]), AclFilter::ids(["b"])]);
        let got: std::collections::BTreeSet<String> = be
            .search(&acl_filter, "deadlock", 10)
            .unwrap()
            .into_iter()
            .map(|h| h.doc_id)
            .collect();
        assert_eq!(got, ["a", "b"].iter().map(|s| s.to_string()).collect());

        let acl_filter = AclFilter::And(vec![AclFilter::All, AclFilter::not_ids(["c"])]);
        let got: std::collections::BTreeSet<String> = be
            .search(&acl_filter, "deadlock", 10)
            .unwrap()
            .into_iter()
            .map(|h| h.doc_id)
            .collect();
        assert_eq!(got, ["a", "b"].iter().map(|s| s.to_string()).collect());

        let acl_filter = AclFilter::And(vec![
            AclFilter::ids(["a", "b"]),
            AclFilter::Not(Box::new(AclFilter::ids(["b"]))),
        ]);
        let got: Vec<String> = be
            .search(&acl_filter, "deadlock", 10)
            .unwrap()
            .into_iter()
            .map(|h| h.doc_id)
            .collect();
        assert_eq!(
            got,
            vec!["a".to_string()],
            "left AND NOT right = the difference, no leak of b"
        );

        let acl_filter = AclFilter::And(vec![]);
        let all_via_empty_and = be.search(&acl_filter, "deadlock", 10).unwrap();
        assert_eq!(
            all_via_empty_and.len(),
            3,
            "empty And ⇒ All (every matching doc)"
        );
        let acl_filter = AclFilter::Or(vec![]);
        let none_via_empty_or = be.search(&acl_filter, "deadlock", 10).unwrap();
        assert!(
            none_via_empty_or.is_empty(),
            "empty Or ⇒ None (nothing visible)"
        );

        let acl_filter = AclFilter::Or(vec![AclFilter::None, AclFilter::ids(["a"])]);
        let got: Vec<String> = be
            .search(&acl_filter, "deadlock", 10)
            .unwrap()
            .into_iter()
            .map(|h| h.doc_id)
            .collect();
        assert_eq!(
            got,
            vec!["a".to_string()],
            "a None sub-clause drops out of the union"
        );
        let acl_filter = AclFilter::And(vec![AclFilter::None, AclFilter::All]);
        assert!(
            be.search(&acl_filter, "deadlock", 10).unwrap().is_empty(),
            "None absorbs the And"
        );
    }

    #[test]
    fn acl_filter_admits_matches_set_semantics() {
        assert!(AclFilter::All.admits("x", "x"));
        assert!(!AclFilter::None.admits("x", "x"));
        assert!(AclFilter::ids(["x"]).admits("x", "x") && !AclFilter::ids(["y"]).admits("x", "x"));
        assert!(
            !AclFilter::not_ids(["x"]).admits("x", "x")
                && AclFilter::not_ids(["y"]).admits("x", "x")
        );
        assert!(AclFilter::And(vec![AclFilter::All, AclFilter::ids(["x"])]).admits("x", "x"));
        assert!(!AclFilter::And(vec![AclFilter::None, AclFilter::All]).admits("x", "x"));
        assert!(AclFilter::Or(vec![AclFilter::None, AclFilter::ids(["x"])]).admits("x", "x"));
        assert!(
            !AclFilter::Or(vec![]).admits("x", "x"),
            "empty Or admits nothing"
        );
        assert!(
            !AclFilter::Or(vec![AclFilter::ids(["y"]), AclFilter::None]).admits("x", "x"),
            "a non-empty Or whose subs all reject the doc admits nothing"
        );
        assert!(AclFilter::Not(Box::new(AclFilter::ids(["y"]))).admits("x", "x"));
        assert!(!AclFilter::Not(Box::new(AclFilter::All)).admits("x", "x"));

        assert!(
            AclFilter::ids(["parent"]).admits("sub", "parent"),
            "a grant on the parent acl_object admits the sub-doc (acl_object arm)"
        );
        assert!(
            AclFilter::ids(["sub"]).admits("sub", "parent"),
            "a grant on the sub-precise doc_id admits it (doc_id arm)"
        );
        assert!(!AclFilter::ids(["other"]).admits("sub", "parent"));
        assert!(
            !AclFilter::not_ids(["parent"]).admits("sub", "parent"),
            "a deny on the parent acl_object excludes the sub-doc (no leak - R2.7)"
        );
        assert!(
            !AclFilter::not_ids(["sub"]).admits("sub", "parent"),
            "a deny on the sub-precise doc_id excludes it (doc_id arm)"
        );
        assert!(AclFilter::not_ids(["other"]).admits("sub", "parent"));
    }

    #[test]
    fn index_error_displays_loudly() {
        let e = IndexError::Engine("boom".into());
        let s = format!("{e}");
        assert!(
            s.contains("boom"),
            "the Display surfaces the underlying engine error"
        );
        assert!(s.contains("index engine error"), "and names it loudly");
        assert!(!s.is_empty(), "never an empty (silent) error message");
    }
}
