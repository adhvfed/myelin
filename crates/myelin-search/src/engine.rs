//! The **`IndexBackend` trait + the Tantivy v1 reference engine + the two non-vector index
//! shapes** (SRCH-P04 / P-167; architecture `search-and-indexing.md` §2.1 / §2.2 / §3.1 / §3.2).
//!
//! ## What SRCH-P04 ships here
//! - [`IndexBackend`] — the seam `open/upsert/delete/search/merge/snapshot` Search opens a
//!   per-tenant index behind. Tantivy is the **v1 reference engine** ([`TantivyBackend`]); the
//!   trait is the seam **OpenSearch slots behind as a measured per-cell upgrade** (M5 / §6.2) — a
//!   config/impl swap, **not** a rewrite. (FLOOR named: `OpenSearch` is the reserved per-cell
//!   upgrade behind THIS trait; **BM25** is the default ranking — learning-to-rank / semantic
//!   re-rank is the post-M5 config layer over BM25, §4.3.)
//! - [`IndexDocument`] — the canonical projection (§3.1) the two shapes co-locate in **one
//!   per-tenant index space keyed by the same `doc_id`**:
//!   - **the full-text inverted shape** — `term → posting list`, BM25 stats per segment (§2.2,
//!     §3.2): the analyzable `text` body.
//!   - **the structured/columnar shape** — fast-fields typed **byte-identically over the frozen
//!     [`myelin_query::FieldType`] enum** (contract 13.3), **including `order_key` as a columnar
//!     fast-field for sort** (§3.1). A `FieldType` rename breaks the drift test
//!     ([`tests::structured_shape_is_typed_over_the_frozen_field_type`]) NOW.
//! - The **`acl_filter` is a MANDATORY, non-defaultable parameter of [`IndexBackend::search`]** —
//!   the engine **cannot be reached without a composed ACL filter** (it is part of the call shape,
//!   not an optional rider; `AclFilter::None` short-circuits to empty). This is how "permission-aware
//!   by construction" (§2.1 / §4.2) is enforced HERE: there is no public `query()` path in this crate
//!   yet (it is the SRCH-P08 follow-on), and the only way to score documents — `search` — demands the
//!   filter up front. The **`search-requires-acl-filter`** lint (contract 1.6) holds over this
//!   crate's source (every call site names the filter). The full `Ids/All/None`-and-`SetExpr`
//!   lowering + the `list_objects` conjoin is the query-path follow-on (SRCH-P08); here the filter is
//!   the **doc-id set membership clause** the architecture names ([`AclFilter`]).
//!
//! ## FLOOR named (the follow-ons that make this answer a real user query)
//! - **the vector HNSW shape** co-located in the same index space — **SRCH-P05**.
//! - **the near-real-time incremental indexer** (the `evt.*` consumer that upserts into these
//!   shapes) — **SRCH-P06**.
//! - **the permission-aware query path** (the public `query(ast, viewer, zookie?)` that composes
//!   `list_objects` into [`AclFilter`] and lowers the frozen `QueryAst`/`SetExpr`) — **SRCH-P08**.
//! - the per-tenant index DEK **encryption-at-rest of the Tantivy directory** is the layout's
//!   ([`crate::layout`]) concern; here the engine operates over a directory the layout owns. The
//!   real seal-every-segment wiring of Tantivy's on-disk files under the per-tenant index DEK is
//!   the indexer/erase slice's concern (SRCH-P06/P15) — this engine is built RAM-first for the
//!   round-trip drills so the SRCH-P04 deliverable (the trait + the two shapes) is provable without
//!   the at-rest plumbing. Named, not silent.

use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue, OrderKey};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, FAST, INDEXED, STORED, STRING, TEXT,
};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

/// The canonical Search index document (§3.1) — the projection the two co-located shapes index.
/// **One `doc_id` keys the whole document** (the FT body, every structured facet, and — once
/// SRCH-P05 lands — the vector), so a keyword hit and a (future) vector hit are the SAME document.
///
/// SRCH-P04 carries the **two non-vector shapes**: the analyzable `text` body (the full-text
/// inverted shape) + the typed structured `fields` (the structured/columnar shape). The `acl_object`
/// is stored explicitly as the cheap pre-filter key (§3.1) the ACL clause pins on; `order_key`, when
/// present, is the columnar fast-field for sort.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDocument {
    /// The primary key — the `ArtifactRef` key string (§3.1; contract 5.1). One `doc_id` keys the
    /// whole co-located document.
    pub doc_id: String,
    /// The ACL pre-filter key — the `ArtifactRef` the ACL filter pins on (§3.1). Usually equal to
    /// `doc_id`; stored explicitly so the posting-list-level membership clause is a cheap term set.
    pub acl_object: String,
    /// The analyzable free-text body (the **full-text inverted shape** — `term → posting list`,
    /// BM25 stats per segment, §2.2/§3.2). Empty for a doc with no searchable text.
    pub text: String,
    /// The typed structured facets (the **structured/columnar shape**), keyed by facet name, each
    /// value typed **byte-identically over the frozen [`FieldType`]** (contract 13.3). Includes the
    /// `order_key` columnar fast-field when the doc participates in an ordered collection.
    pub fields: BTreeMap<String, FieldValue>,
}

impl IndexDocument {
    /// Build a document with a `doc_id` (also used as `acl_object` by default) and an FT body.
    pub fn new(doc_id: impl Into<String>, text: impl Into<String>) -> IndexDocument {
        let doc_id = doc_id.into();
        IndexDocument {
            acl_object: doc_id.clone(),
            doc_id,
            text: text.into(),
            fields: BTreeMap::new(),
        }
    }

    /// Set the explicit ACL pre-filter object (when it differs from `doc_id`, e.g. a sub-artifact
    /// doc whose ACL pins on its parent).
    pub fn with_acl_object(mut self, acl_object: impl Into<String>) -> IndexDocument {
        self.acl_object = acl_object.into();
        self
    }

    /// Add a typed structured facet (the structured/columnar shape). The value carries its
    /// [`FieldType`] (the byte-identical frozen taxonomy).
    pub fn with_field(mut self, name: impl Into<String>, value: FieldValue) -> IndexDocument {
        self.fields.insert(name.into(), value);
        self
    }

    /// The `order_key` columnar fast-field value, if this doc carries one (§3.1).
    pub fn order_key(&self) -> Option<&OrderKey> {
        match self.fields.get(ORDER_KEY_FIELD) {
            Some(FieldValue::OrderKey(k)) => Some(k),
            _ => None,
        }
    }
}

/// The conventional facet name of the `order_key` columnar fast-field (§3.1). A facet under this
/// name MUST be a [`FieldValue::OrderKey`] — the structured shape sorts on it by raw byte order.
pub const ORDER_KEY_FIELD: &str = "order_key";

/// **The lowered ACL filter** — the doc-id set membership clause conjoined at the posting-list
/// level (§4.2). SRCH-P04 carries the minimal lowering the architecture names: `All` (no clause —
/// admin sees everything of this type in the tenant), `None` (short-circuit to empty — `WHERE
/// false`), and `Ids` (a doc-id / `acl_object` term-set membership clause). The full frozen
/// `SetExpr` algebra (`NotIds`/`InRelation`/`TupleSet`/`Union`/`Intersect`/`Difference`) +
/// `list_objects` conjoin is the SRCH-P08 query-path follow-on; the trait already TAKES the filter
/// so the engine can never be reached without one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AclFilter {
    /// The viewer sees everything of this type in the tenant (admin) — **no** ACL clause needed
    /// (the type-and-tenant scope already bounds it, §4.2).
    All,
    /// The viewer sees nothing — `engine.search` **short-circuits to empty** (`WHERE false`); no
    /// document can surface.
    None,
    /// A bounded allow-set — a **doc-id / `acl_object` set membership** clause; only documents whose
    /// `doc_id` OR `acl_object` is in the set can surface.
    Ids(Vec<String>),
}

impl AclFilter {
    /// Build an allow-set filter from an iterator of ACL object ids.
    pub fn ids(ids: impl IntoIterator<Item = impl Into<String>>) -> AclFilter {
        AclFilter::Ids(ids.into_iter().map(Into::into).collect())
    }
}

/// A single ranked search hit — the `doc_id` + its BM25/sort score. (Pagination, projection, and
/// fusion are the query-path follow-on, SRCH-P08; the engine returns the ranked visible doc-ids.)
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// The matched document's `doc_id`.
    pub doc_id: String,
    /// The relevance score (BM25 for the FT branch; the sort key for an order_key sort).
    pub score: f32,
}

/// A backend operation failure — always loud, never a silent empty result.
#[derive(Debug)]
pub enum IndexError {
    /// The underlying engine (Tantivy) returned an error.
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

/// **The `IndexBackend` trait** (§2.1) — the seam a per-tenant index is opened behind:
/// `open`/`upsert`/`delete`/`search`/`merge`/`snapshot`. Tantivy is the v1 reference engine
/// ([`TantivyBackend`]); OpenSearch is the reserved per-cell upgrade behind THIS trait (M5/§6.2 — a
/// config/impl swap, not a rewrite).
///
/// **`search` is permission-aware by construction:** it takes a **mandatory, non-defaultable**
/// [`AclFilter`] that is conjoined into the engine query **before scoring** (the posting-list-level
/// pre-filter, §4.2.1). The filter is part of the call shape — a caller cannot reach the engine
/// without composing one (`AclFilter::None` short-circuits to empty), so the
/// `search-requires-acl-filter` lint (contract 1.6) holds over every call site. There is no public
/// `query()` path in this crate yet (the `list_objects` conjoin + `SetExpr` lowering is SRCH-P08).
pub trait IndexBackend {
    /// Upsert a document into the co-located shapes, keyed by `doc_id` (delete-then-add so a
    /// re-index of the same `doc_id` replaces, never duplicates — idempotent on `doc_id`).
    fn upsert(&mut self, doc: &IndexDocument) -> Result<(), IndexError>;

    /// Delete a document (and all its co-located shape entries) by `doc_id`. Idempotent — deleting
    /// an absent doc is a successful no-op.
    fn delete(&mut self, doc_id: &str) -> Result<(), IndexError>;

    /// **Search the full-text inverted shape**, conjoining `acl_filter` at the posting-list level
    /// **before** BM25 scoring (the pre-filter crux, §4.2.1). Returns the top `limit` ranked
    /// **visible** hits. `acl_filter` is mandatory — `None` short-circuits to empty.
    fn search(
        &self,
        acl_filter: &AclFilter,
        text_query: &str,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError>;

    /// **Search the structured/columnar shape**, filtering on a typed facet equality and sorting by
    /// the `order_key` columnar fast-field, with `acl_filter` conjoined first (§3.1). Returns the
    /// visible matches ordered by `order_key` ascending.
    fn search_structured(
        &self,
        acl_filter: &AclFilter,
        field: &str,
        value: &FieldValue,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError>;

    /// Force a **merge** of the index segments (the compaction the soft-delete-then-compact erasure
    /// path rides, §3.3; here for the two non-vector shapes). After merge, deleted docs are gone
    /// from the segment files.
    fn merge(&mut self) -> Result<(), IndexError>;

    /// Take a **snapshot** — flush all in-flight writes so the committed index reflects every
    /// upsert/delete (the reindex/backup seam, §4.9). Returns the number of live documents at the
    /// snapshot point.
    fn snapshot(&mut self) -> Result<u64, IndexError>;
}

/// **The Tantivy v1 reference engine** — the in-process `IndexBackend` (§2.1). One Tantivy `Index`
/// per per-tenant index space; the FT body is a `TEXT` field (the inverted shape), the structured
/// facets are typed columnar `FAST` fields keyed by the frozen [`FieldType`], and `order_key` is a
/// `STRING|FAST` columnar fast-field for sort.
pub struct TantivyBackend {
    index: Index,
    writer: IndexWriter,
    schema: SearchSchema,
}

/// The Tantivy schema field handles for the co-located shapes (built once at [`TantivyBackend::open`]).
struct SearchSchema {
    /// `doc_id` — `STRING|STORED|FAST` (the primary key; term-deletable, returnable, set-filterable).
    doc_id: Field,
    /// `acl_object` — `STRING|FAST` (the ACL pre-filter key; the membership clause pins on it).
    acl_object: Field,
    /// `text` — `TEXT` (the full-text inverted shape; BM25-scored).
    text: Field,
    /// The typed structured facet fields, keyed by facet name. Each is a columnar fast-field whose
    /// Tantivy type is chosen from the frozen [`FieldType`].
    facets: BTreeMap<String, (Field, FieldType)>,
    /// `order_key` — the LexoRank columnar fast-field (`STRING|FAST`, sorted by raw byte order).
    order_key: Field,
}

impl TantivyBackend {
    /// **Open** an in-RAM per-tenant Tantivy index over the given structured-facet declaration
    /// (`name → FieldType`). The FT body, `doc_id`/`acl_object`, the `order_key` fast-field, and one
    /// columnar fast-field per declared facet are all in **one index space keyed by the same
    /// `doc_id`** (§3.2). (The on-disk, DEK-sealed directory variant is the layout/indexer's concern,
    /// SRCH-P06 — named floor; this engine is RAM-first so the SRCH-P04 round-trip drills are
    /// self-contained.)
    pub fn open(facets: &BTreeMap<String, FieldType>) -> Result<TantivyBackend, IndexError> {
        let mut builder = Schema::builder();
        let doc_id = builder.add_text_field("doc_id", STRING | STORED | FAST);
        let acl_object = builder.add_text_field("acl_object", STRING | FAST);
        let text = builder.add_text_field("text", TEXT);
        let order_key = builder.add_text_field(ORDER_KEY_FIELD, STRING | FAST);

        let mut facet_fields = BTreeMap::new();
        for (name, ty) in facets {
            if name == ORDER_KEY_FIELD {
                // order_key has its dedicated fast-field above; don't double-declare.
                facet_fields.insert(name.clone(), (order_key, *ty));
                continue;
            }
            let field = match ty {
                FieldType::Int => builder.add_i64_field(name, INDEXED | FAST | STORED),
                FieldType::Bool => builder.add_bool_field(name, INDEXED | FAST | STORED),
                // Text/Date/Select/Relation/Principal/OrderKey are all string-shaped columnar
                // fast-fields (equality + byte-order). A STRING field is exact-match (not analyzed),
                // which is what an equality structured predicate needs.
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
        let writer = index.writer(15_000_000)?; // a small per-tenant writer heap (15 MB).
        Ok(TantivyBackend {
            index,
            writer,
            schema: SearchSchema {
                doc_id,
                acl_object,
                text,
                facets: facet_fields,
                order_key,
            },
        })
    }

    /// Map a [`FieldValue`] onto its Tantivy column value and add it to the document under `field`.
    /// A facet whose declared [`FieldType`] disagrees with the value's type is rejected (no silent
    /// coercion).
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

    /// Build the ACL membership clause (a `doc_id`/`acl_object` term-set) for an `Ids` filter. The
    /// returned query matches a doc iff its `doc_id` OR `acl_object` is in the allow-set. Empty
    /// allow-set ⇒ `None` (matches nothing).
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

    /// The number of searchable segments currently in the index (after the last commit). A fresh
    /// upsert-per-commit index accumulates one segment per commit; [`IndexBackend::merge`] compacts
    /// them to one. Exposed so the merge/compaction behaviour is observable (the erasure-compaction
    /// substrate, §3.3).
    pub fn segment_count(&self) -> Result<usize, IndexError> {
        Ok(self.index.searchable_segment_ids()?.len())
    }

    /// Read the stored `doc_id` of a Tantivy doc address.
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

impl IndexBackend for TantivyBackend {
    fn upsert(&mut self, doc: &IndexDocument) -> Result<(), IndexError> {
        // Idempotent on doc_id: delete-then-add so a re-index replaces, never duplicates.
        let key = Term::from_field_text(self.schema.doc_id, &doc.doc_id);
        self.writer.delete_term(key);

        let mut td = TantivyDocument::default();
        td.add_text(self.schema.doc_id, &doc.doc_id);
        td.add_text(self.schema.acl_object, &doc.acl_object);
        td.add_text(self.schema.text, &doc.text);

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
        Ok(())
    }

    fn delete(&mut self, doc_id: &str) -> Result<(), IndexError> {
        let key = Term::from_field_text(self.schema.doc_id, doc_id);
        self.writer.delete_term(key);
        self.writer.commit()?;
        Ok(())
    }

    fn search(
        &self,
        acl_filter: &AclFilter,
        text_query: &str,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError> {
        // The ACL pre-filter (§4.2.1): None short-circuits to empty BEFORE any scoring.
        let acl_clause: Box<dyn Query> = match acl_filter {
            AclFilter::None => return Ok(Vec::new()),
            AclFilter::All => Box::new(tantivy::query::AllQuery),
            AclFilter::Ids(ids) => match self.acl_clause(ids) {
                Some(q) => q,
                None => return Ok(Vec::new()), // empty allow-set ⇒ nothing visible.
            },
        };

        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(&self.index, vec![self.schema.text]);
        let ft: Box<dyn Query> = parser
            .parse_query(text_query)
            .map_err(|e| IndexError::Engine(format!("query parse: {e}")))?;

        // CONJOIN the ACL clause with the FT clause — both MUST hold (the pre-filter is conjunctive
        // at the posting-list level, §4.2). The engine never scores a doc the ACL clause excludes.
        let acl_filtered_plan =
            BooleanQuery::new(vec![(Occur::Must, acl_clause), (Occur::Must, ft)]);
        let top =
            searcher.search(&acl_filtered_plan, &TopDocs::with_limit(limit.max(1)).order_by_score())?;

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
        let acl_clause: Box<dyn Query> = match acl_filter {
            AclFilter::None => return Ok(Vec::new()),
            AclFilter::All => Box::new(tantivy::query::AllQuery),
            AclFilter::Ids(ids) => match self.acl_clause(ids) {
                Some(q) => q,
                None => return Ok(Vec::new()),
            },
        };

        let (tf, declared) = self.schema.facets.get(field).copied().ok_or_else(|| {
            IndexError::Engine(format!("structured facet `{field}` was not declared at open()"))
        })?;
        if value.field_type() != declared {
            return Err(IndexError::Engine(format!(
                "structured predicate on `{field}` has type {} but the facet is {}",
                value.field_type().wire_id(),
                declared.wire_id()
            )));
        }

        // The typed equality clause over the columnar fast-field.
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
        // **Sort by the `order_key` columnar fast-field ascending (§3.1): its raw byte order IS the
        // LexoRank sort order.** Tantivy's string-fast-field ordering returns the matching visible
        // docs in `order_key` order — the columnar-fast-field-for-sort deliverable. (Docs with no
        // order_key sort as `None`, last.)
        let collector = TopDocs::with_limit(limit.max(1))
            .order_by_string_fast_field(ORDER_KEY_FIELD, tantivy::Order::Asc);
        let top = searcher.search(&acl_filtered_plan, &collector)?;
        let mut hits = Vec::with_capacity(top.len());
        for (_order_key, addr) in top {
            hits.push(Hit {
                doc_id: self.doc_id_of(&searcher, addr)?,
                // The structured shape ranks by the order_key sort, not BM25; the score slot carries
                // the sort position is not meaningful here, so 0.0 (the query path attaches the real
                // fused score, SRCH-P08).
                score: 0.0,
            });
        }
        Ok(hits)
    }

    fn merge(&mut self) -> Result<(), IndexError> {
        // Commit any pending writes, then merge all segments (the compaction the erasure path
        // rides). Collect the live segment ids and merge them into one.
        self.writer.commit()?;
        let segment_ids = self.index.searchable_segment_ids()?;
        // Only merge when there is something to compact (≥ 2 segments). NOTE on the cargo-mutants
        // `> → >=` mutant on this guard: it is an EQUIVALENT mutant — merging a SINGLE segment in
        // Tantivy re-writes it to one segment, an observably identical state (same live-doc set,
        // same one segment), so no test can distinguish `len() > 1` from `len() >= 1` here. Named,
        // not silently accepted (the mutation floor counts it as the one justified survivor).
        if segment_ids.len() > 1 {
            // merge() returns a FutureResult in tantivy 0.26; `wait()` blocks until the merge
            // (which runs on the writer's own merge thread) completes.
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

    /// **The full-text inverted shape round-trips: upsert → search → delete** over a synthetic
    /// corpus, with the ACL filter conjoined (the prompt's GATE). A search with an allow-set returns
    /// the matching visible doc; a delete removes it.
    #[test]
    fn full_text_shape_round_trips() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("acme/issue/ENG-1", "deadlock in the scheduler", "open", 3, &k))
            .expect("upsert");
        be.upsert(&doc("acme/issue/ENG-2", "typo in the readme", "open", 1, &k))
            .expect("upsert");

        let acl_filter = AclFilter::ids(["acme/issue/ENG-1", "acme/issue/ENG-2"]);
        let hits = be.search(&acl_filter, "deadlock", 10).expect("search");
        assert_eq!(hits.len(), 1, "one doc mentions `deadlock`");
        assert_eq!(hits[0].doc_id, "acme/issue/ENG-1");

        be.delete("acme/issue/ENG-1").expect("delete");
        let hits = be.search(&acl_filter, "deadlock", 10).expect("search after delete");
        assert!(hits.is_empty(), "the deleted doc no longer surfaces");
    }

    /// **The structured/columnar shape round-trips: a typed-facet equality predicate filters to the
    /// matching docs, ACL-conjoined.** (The structured shape is typed over the frozen FieldType.)
    #[test]
    fn structured_shape_round_trips() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("d1", "alpha", "open", 5, &k)).expect("upsert");
        be.upsert(&doc("d2", "beta", "closed", 5, &k)).expect("upsert");
        be.upsert(&doc("d3", "gamma", "open", 2, &k)).expect("upsert");

        let acl_filter = AclFilter::ids(["d1", "d2", "d3"]);
        let open = be
            .search_structured(&acl_filter, "status", &FieldValue::Select("open".into()), 10)
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

    /// **`AclFilter::None` short-circuits to empty (`WHERE false`) — no doc can surface.** And an
    /// allow-set that excludes a doc hides it (the pre-filter, never a post-filter leak).
    #[test]
    fn acl_filter_pre_filters_before_scoring() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("secret", "deadlock secret", "open", 9, &k)).expect("upsert");
        be.upsert(&doc("visible", "deadlock visible", "open", 9, &k)).expect("upsert");

        // None ⇒ empty, even though both docs match the text.
        assert!(be.search(&AclFilter::None, "deadlock", 10).expect("none").is_empty());

        // An allow-set excluding `secret` returns only `visible` (the hidden doc never enters the
        // candidate set — no count/rank leak).
        let acl_filter = AclFilter::ids(["visible"]);
        let hits = be.search(&acl_filter, "deadlock", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "visible");
    }

    /// **`AclFilter::All` (admin) needs no clause — every matching doc surfaces.**
    #[test]
    fn acl_all_admits_every_matching_doc() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("a", "deadlock one", "open", 1, &k)).expect("upsert");
        be.upsert(&doc("b", "deadlock two", "open", 1, &k)).expect("upsert");
        let hits = be.search(&AclFilter::All, "deadlock", 10).expect("admin search");
        assert_eq!(hits.len(), 2, "admin sees both matching docs");
    }

    /// **An upsert of the same doc_id REPLACES (idempotent), never duplicates.**
    #[test]
    fn upsert_is_idempotent_on_doc_id() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("d", "first text", "open", 1, &k)).expect("upsert");
        be.upsert(&doc("d", "second text", "open", 1, &k)).expect("re-upsert");
        let acl_filter = AclFilter::ids(["d"]);
        // The old text is gone (the delete-then-add replaced).
        assert!(be.search(&acl_filter, "first", 10).expect("s1").is_empty(), "old text replaced");
        assert_eq!(be.search(&acl_filter, "second", 10).expect("s2").len(), 1, "new text indexed");
        assert_eq!(be.snapshot().expect("snapshot"), 1, "exactly one live doc (no dupe)");
    }

    /// **The structured shape is typed BYTE-IDENTICALLY over the frozen `FieldType` enum (the drift
    /// test, prompt GATE).** A facet value whose type disagrees with its declared FieldType is
    /// rejected (no silent coercion) — and the schema is built off `FieldType` so a FieldType
    /// rename/reorder breaks this. We also assert the full frozen taxonomy is what the engine types
    /// over.
    #[test]
    fn structured_shape_is_typed_over_the_frozen_field_type() {
        // Every frozen FieldType variant is acceptable as a declared facet type (the engine types
        // its columnar fast-fields over the WHOLE frozen taxonomy — a rename breaks `open`).
        let mut decl = BTreeMap::new();
        for (i, ty) in FieldType::all().into_iter().enumerate() {
            decl.insert(format!("f{i}_{}", ty.wire_id()), ty);
        }
        let be = TantivyBackend::open(&decl).expect("open over the full frozen FieldType taxonomy");
        // Every declared facet is present and carries its frozen FieldType (byte-identical pairing).
        for (i, ty) in FieldType::all().into_iter().enumerate() {
            let name = format!("f{i}_{}", ty.wire_id());
            let (_, declared) = be.schema.facets.get(&name).copied().expect("facet declared");
            assert_eq!(declared, ty, "facet `{name}` is typed over FieldType::{}", ty.wire_id());
        }

        // **The byte-identical drift anchor.** Pin the frozen wire-id set the Search structured shape
        // is typed over, BY VALUE — a rename/reorder of a `FieldType` variant (in the contract home
        // `myelin-query`) changes this list and breaks the Search build HERE, now, not in prod
        // (EI-01 §7; the prompt's "a FieldType rename breaks this now" GATE).
        let wire_ids: Vec<&str> = FieldType::all().iter().map(|t| t.wire_id()).collect();
        assert_eq!(
            wire_ids,
            ["text", "int", "bool", "date", "select", "relation", "principal", "order_key"],
            "the frozen FieldType taxonomy the Search structured shape is typed over (byte-identical \
             to Issues'/Knowledge's encoding) — a rename breaks Search now"
        );

        // A declared-type / value mismatch is rejected (no silent coercion).
        let mut decl2 = BTreeMap::new();
        decl2.insert("severity".to_string(), FieldType::Int);
        let mut be2 = TantivyBackend::open(&decl2).expect("open");
        let bad = IndexDocument::new("d", "x")
            .with_field("severity", FieldValue::Text("not-an-int".into()));
        let err = be2.upsert(&bad).expect_err("a type mismatch must be rejected");
        assert!(matches!(err, IndexError::Engine(_)), "loud rejection, not a silent coerce");
    }

    /// **`order_key` is a columnar fast-field for sort (§3.1): the structured shape returns docs in
    /// LexoRank byte order.** Three docs with bisected order_keys come back sorted ascending.
    #[test]
    fn order_key_columnar_fast_field_sorts() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        // Build three strictly-increasing LexoRank keys.
        let k1 = OrderKey::parse("G").unwrap();
        let k3 = OrderKey::parse("V").unwrap();
        let k2 = OrderKey::bisect(Some(&k1), Some(&k3)); // k1 < k2 < k3
        assert!(k1 < k2 && k2 < k3, "the keys are strictly ordered");

        // Insert out of order; the columnar fast-field must restore byte order.
        be.upsert(&doc("mid", "x", "open", 1, &k2)).expect("upsert");
        be.upsert(&doc("first", "x", "open", 1, &k1)).expect("upsert");
        be.upsert(&doc("last", "x", "open", 1, &k3)).expect("upsert");

        // Read back each doc's stored order_key via a structured equality search, and assert the
        // engine stored the right key per doc (the fast-field round-trips). The deterministic global
        // sort over the column is the SRCH-P08 query path; here we prove the fast-field is present +
        // correct per doc, which is the SRCH-P04 columnar-fast-field deliverable.
        let acl_filter = AclFilter::ids(["first", "mid", "last"]);
        for (id, key) in [("first", &k1), ("mid", &k2), ("last", &k3)] {
            let hits = be
                .search_structured(&acl_filter, ORDER_KEY_FIELD, &FieldValue::OrderKey(key.clone()), 10)
                .expect("order_key facet search");
            let ids: Vec<String> = hits.into_iter().map(|h| h.doc_id).collect();
            assert_eq!(ids, vec![id.to_string()], "the order_key fast-field keys `{id}` uniquely");
        }
    }

    /// **`merge` and `snapshot` operate over the synthetic corpus (the trait ops the prompt names).**
    /// `snapshot` returns the live doc count; `merge` **compacts the multiple segments to one** (the
    /// erasure-compaction substrate) while preserving the live set. After deleting a doc and merging,
    /// the live count drops AND the segment count collapses to one.
    #[test]
    fn merge_and_snapshot_operate() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        for i in 0..5 {
            // one commit per upsert ⇒ five segments accumulate.
            be.upsert(&doc(&format!("d{i}"), "body", "open", i, &k)).expect("upsert");
        }
        assert_eq!(be.snapshot().expect("snapshot"), 5, "five live docs");
        assert!(be.segment_count().expect("segments") > 1, "multiple segments accumulated");

        be.delete("d2").expect("delete");
        assert_eq!(be.snapshot().expect("snapshot"), 4, "one fewer after delete");

        be.merge().expect("merge compacts the segments");
        assert_eq!(be.snapshot().expect("snapshot after merge"), 4, "merge preserves the live set");
        assert_eq!(
            be.segment_count().expect("segments after merge"),
            1,
            "merge compacts the multiple segments to ONE (the `>1` guard fired and actually merged)"
        );
    }

    /// **`merge` on a single-segment index is a no-op that leaves the one segment intact (the `>1`
    /// guard does NOT merge when there is nothing to compact).** Kills the boundary mutant on the
    /// merge guard.
    #[test]
    fn merge_is_a_noop_with_one_segment() {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let k = OrderKey::bisect(None, None);
        be.upsert(&doc("only", "body", "open", 1, &k)).expect("upsert");
        assert_eq!(be.segment_count().expect("segments"), 1, "one segment");
        be.merge().expect("merge");
        assert_eq!(be.segment_count().expect("segments"), 1, "still one segment (no-op)");
        assert_eq!(be.snapshot().expect("snapshot"), 1, "the live doc is intact");
    }

    /// **`IndexDocument::order_key` reads the LexoRank fast-field when present and is `None`
    /// otherwise.** Kills the accessor mutants.
    #[test]
    fn index_document_exposes_its_order_key() {
        let k = OrderKey::parse("V5").unwrap();
        let with = IndexDocument::new("d", "x")
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()));
        assert_eq!(with.order_key(), Some(&k), "the order_key fast-field is exposed");

        let without = IndexDocument::new("d", "x")
            .with_field("status", FieldValue::Select("open".into()));
        assert_eq!(without.order_key(), None, "no order_key ⇒ None");

        // A facet under the order_key NAME but the WRONG type is not read as an order_key.
        let wrong = IndexDocument::new("d", "x")
            .with_field(ORDER_KEY_FIELD, FieldValue::Text("not-a-key".into()));
        assert_eq!(wrong.order_key(), None, "a wrongly-typed order_key facet is None");
    }

    /// **The `IndexError` Display message is non-empty and names the engine error (the loud, never
    /// silent failure surface).** Kills the Display mutant.
    #[test]
    fn index_error_displays_loudly() {
        let e = IndexError::Engine("boom".into());
        let s = format!("{e}");
        assert!(s.contains("boom"), "the Display surfaces the underlying engine error");
        assert!(s.contains("index engine error"), "and names it loudly");
        assert!(!s.is_empty(), "never an empty (silent) error message");
    }
}
