//! §P5-A pa5.2: tantivy-backed full-text index. Behind the `tantivy` feature
//! flag. Selected at runtime via `RUSTY_RED_FULLTEXT_BACKEND=tantivy` through
//! the `crate::fulltext::make_fulltext_backend` factory.
//!
//! The hand-rolled BM25 in `fulltext.rs` remains the default; tantivy is the
//! perf-oriented alternative the original SPEC named. Both implement
//! `FullTextBackend` so the rest of the system reads them through a uniform
//! interface.
//!
//! Implementation notes:
//! - Index is created in RAM (`Index::create_in_ram`) so no disk side effects
//!   leak from this layer; persistence is the caller's concern.
//! - We `commit()` after every upsert/remove so the trait's "write-then-read"
//!   contract holds. That matches the hand-rolled backend's instant-visibility
//!   semantics. Callers that batch many writes can drop down to the raw
//!   `tantivy::IndexWriter` if needed, but the trait surface is correctness-
//!   first.

use tantivy::{
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{Field, Schema, Value, STORED, STRING, TEXT},
    Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term,
};

use crate::fulltext::{FullTextBackend, FullTextDesignation};

/// Tantivy 0.22's documented minimum is 15 MB; we sit right at that floor so
/// many concurrent designations don't multiply by a much larger per-writer
/// arena. A future bulk-upsert path can share writers across designations.
const WRITER_MEMORY_BUDGET: usize = 15_000_000;

pub struct TantivyFullTextBackend {
    designation: FullTextDesignation,
    index: Index,
    writer: IndexWriter,
    reader: IndexReader,
    doc_id_field: Field,
    text_field: Field,
}

impl std::fmt::Debug for TantivyFullTextBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TantivyFullTextBackend")
            .field("designation", &self.designation)
            .finish_non_exhaustive()
    }
}

impl TantivyFullTextBackend {
    /// Returns either an initialized backend or a human-readable init error
    /// string (the factory wraps this in `FullTextBackendError::TantivyInit`).
    pub fn new(designation: FullTextDesignation) -> Result<Self, String> {
        let mut schema_builder = Schema::builder();
        let doc_id_field = schema_builder.add_text_field("doc_id", STRING | STORED);
        let text_field = schema_builder.add_text_field("text", TEXT);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let writer = index
            .writer(WRITER_MEMORY_BUDGET)
            .map_err(|err| format!("could not create tantivy IndexWriter: {err}"))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|err| format!("could not build tantivy reader: {err}"))?;
        Ok(Self {
            designation,
            index,
            writer,
            reader,
            doc_id_field,
            text_field,
        })
    }
}

impl TantivyFullTextBackend {
    /// Delete any existing doc with the given id; STRING tokenization on
    /// `doc_id_field` keeps the term match exact. The companion `commit_now`
    /// flushes to a segment so the next `searcher()` sees the change — that
    /// matches the `FullTextBackend` write-then-read contract.
    fn delete_doc_id(&mut self, doc_id: &str) {
        let term = Term::from_field_text(self.doc_id_field, doc_id);
        let _ = self.writer.delete_term(term);
    }

    /// commit-per-write keeps the `FullTextBackend` contract honest; the
    /// tradeoff (commit() is hundreds of ms per write) is documented at the
    /// top of the file.
    fn commit_now(&mut self) {
        let _ = self.writer.commit();
        let _ = self.reader.reload();
    }
}

impl FullTextBackend for TantivyFullTextBackend {
    fn upsert(&mut self, doc_id: &str, text: &str) {
        self.delete_doc_id(doc_id);
        let _ = self.writer.add_document(doc!(
            self.doc_id_field => doc_id,
            self.text_field => text,
        ));
        self.commit_now();
    }

    fn remove(&mut self, doc_id: &str) {
        self.delete_doc_id(doc_id);
        self.commit_now();
    }

    fn search(&self, query: &str, k: usize) -> Vec<(String, f32)> {
        if query.trim().is_empty() || k == 0 {
            return Vec::new();
        }
        let searcher = self.reader.searcher();
        let parser = QueryParser::for_index(&self.index, vec![self.text_field]);
        let parsed = match parser.parse_query(query) {
            Ok(q) => q,
            Err(_) => return Vec::new(),
        };
        let top = match searcher.search(&parsed, &TopDocs::with_limit(k)) {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let Ok(doc) = searcher.doc::<TantivyDocument>(addr) else {
                continue;
            };
            // `get_first` returns an `OwnedValue` ref; tantivy 0.22 exposes
            // `as_str()` on text values.
            if let Some(value) = doc.get_first(self.doc_id_field) {
                if let Some(text) = value.as_str() {
                    out.push((text.to_string(), score));
                }
            }
        }
        out
    }

    fn designation(&self) -> &FullTextDesignation {
        &self.designation
    }

    fn doc_count(&self) -> usize {
        let searcher = self.reader.searcher();
        searcher.num_docs() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn designation() -> FullTextDesignation {
        FullTextDesignation {
            label: "Doc".into(),
            property: "text".into(),
        }
    }

    #[test]
    fn tantivy_backend_basic_upsert_and_search() {
        let mut backend = TantivyFullTextBackend::new(designation()).unwrap();
        backend.upsert("d1", "the quick brown fox jumps");
        backend.upsert("d2", "the lazy dog sleeps");
        let hits = backend.search("fox", 5);
        assert!(!hits.is_empty(), "expected at least one hit for 'fox'");
        assert_eq!(hits[0].0, "d1");
        assert_eq!(backend.doc_count(), 2);
    }

    #[test]
    fn tantivy_backend_remove_invalidates_doc() {
        let mut backend = TantivyFullTextBackend::new(designation()).unwrap();
        backend.upsert("d1", "rust is great");
        backend.upsert("d2", "tantivy is great");
        backend.remove("d1");
        let hits = backend.search("rust", 5);
        let hit_ids: Vec<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        assert!(!hit_ids.contains(&"d1"), "d1 should be removed");
        // doc_count should reflect the remaining live doc.
        assert_eq!(backend.doc_count(), 1);
    }

    #[test]
    fn tantivy_backend_upsert_replaces_existing_doc() {
        let mut backend = TantivyFullTextBackend::new(designation()).unwrap();
        backend.upsert("d1", "knowledge graph database");
        backend.upsert("d1", "weather forecast for tomorrow");
        // The original "knowledge" content is gone; only "weather" should hit.
        let hits = backend.search("knowledge", 5);
        assert!(hits.iter().all(|(id, _)| id != "d1"));
        let hits = backend.search("weather", 5);
        assert!(hits.iter().any(|(id, _)| id == "d1"));
    }

    #[test]
    fn tantivy_backend_designation_round_trips() {
        let backend = TantivyFullTextBackend::new(designation()).unwrap();
        assert_eq!(backend.designation().label, "Doc");
        assert_eq!(backend.designation().property, "text");
    }
}
