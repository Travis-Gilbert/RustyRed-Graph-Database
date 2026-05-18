//! Phase 5: Inverted-index BM25 full-text search.
//!
//! A purpose-built lexical index keyed by (label, property). We tokenize on
//! non-alphanumeric boundaries, lowercase, and skip stop words. Each
//! `FullTextIndex` holds:
//!   - postings: term -> Vec<(doc_id, term_freq)>
//!   - doc_lengths: doc_id -> u32
//!   - avg_doc_length
//!
//! Scoring is BM25 with k1 = 1.2, b = 0.75 (standard defaults).

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// §P5-A pa5.3: env var that selects the fulltext backend at construction time.
pub const RUSTY_RED_FULLTEXT_BACKEND_ENV: &str = "RUSTY_RED_FULLTEXT_BACKEND";

// Canonical backend-name strings; the dispatcher accepts a few aliases per kind.
pub(crate) const FULLTEXT_BACKEND_HAND_ROLLED: &str = "hand_rolled";
pub(crate) const FULLTEXT_BACKEND_TANTIVY: &str = "tantivy";

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FullTextDesignation {
    pub label: String,
    pub property: String,
}

/// §P5-A pa5.1 + cross-cutting cc.3: the full-text designation is now a
/// required, owned field. The per-tenant map already keys by (label, property),
/// so wrapping the designation in `Option` was redundant and prone to silent
/// fall-throughs ("missing designation" only surfaced via `.unwrap_or_default`).
#[derive(Debug)]
pub struct FullTextIndex {
    pub designation: FullTextDesignation,
    /// term -> list of (doc_id, term frequency in this doc)
    postings: HashMap<String, Vec<(String, u32)>>,
    /// per-doc total term count
    doc_lengths: HashMap<String, u32>,
    /// per-doc unique terms (for O(doc_terms) removes instead of full vocab scan)
    doc_terms: HashMap<String, Vec<String>>,
    /// docs known to this index (for re-indexing on update)
    indexed: BTreeSet<String>,
    total_length: u64,
}

/// §P5-A pa5.1: trait abstraction over the full-text storage layer. The
/// hand-rolled `FullTextIndex` is the only impl today; a tantivy-backed
/// implementation will sit behind a `tantivy` feature flag once the SPEC's
/// switch is implemented.
pub trait FullTextBackend: Send + Sync + std::fmt::Debug {
    /// Index (or re-index) a document under `doc_id` with the given text.
    fn upsert(&mut self, doc_id: &str, text: &str);
    /// Remove a document from the index.
    fn remove(&mut self, doc_id: &str);
    /// Return the top-k scored hits for the query.
    fn search(&self, query: &str, k: usize) -> Vec<(String, f32)>;
    /// Return the designation this backend was created for.
    fn designation(&self) -> &FullTextDesignation;
    /// Number of documents currently indexed.
    fn doc_count(&self) -> usize;
}

impl FullTextIndex {
    pub fn for_designation(d: FullTextDesignation) -> Self {
        Self {
            designation: d,
            postings: HashMap::new(),
            doc_lengths: HashMap::new(),
            doc_terms: HashMap::new(),
            indexed: BTreeSet::new(),
            total_length: 0,
        }
    }

    pub fn doc_count(&self) -> usize {
        self.indexed.len()
    }

    pub fn upsert(&mut self, doc_id: &str, text: &str) {
        if self.indexed.contains(doc_id) {
            self.remove(doc_id);
        }
        let tokens = tokenize(text);
        if tokens.is_empty() {
            // Mark as indexed-with-empty so later removes know about it.
            self.indexed.insert(doc_id.to_string());
            self.doc_lengths.insert(doc_id.to_string(), 0);
            return;
        }

        let mut term_freq: HashMap<String, u32> = HashMap::new();
        for tok in tokens.iter() {
            *term_freq.entry(tok.clone()).or_insert(0) += 1;
        }
        let length = tokens.len() as u32;
        let unique_terms: Vec<String> = term_freq.keys().cloned().collect();
        for (term, freq) in term_freq {
            self.postings
                .entry(term)
                .or_default()
                .push((doc_id.to_string(), freq));
        }
        self.doc_lengths.insert(doc_id.to_string(), length);
        self.doc_terms.insert(doc_id.to_string(), unique_terms);
        self.indexed.insert(doc_id.to_string());
        self.total_length += length as u64;
    }

    pub fn remove(&mut self, doc_id: &str) {
        if !self.indexed.remove(doc_id) {
            return;
        }
        if let Some(len) = self.doc_lengths.remove(doc_id) {
            self.total_length = self.total_length.saturating_sub(len as u64);
        }
        // Only scan the doc's own terms, not the entire vocabulary.
        if let Some(terms) = self.doc_terms.remove(doc_id) {
            for term in terms {
                if let Some(plist) = self.postings.get_mut(&term) {
                    plist.retain(|(id, _)| id != doc_id);
                    if plist.is_empty() {
                        self.postings.remove(&term);
                    }
                }
            }
        }
    }

    /// Return the top-k doc_ids ranked by BM25 against the query string.
    pub fn search(&self, query: &str, k: usize) -> Vec<(String, f32)> {
        let n = self.indexed.len() as f64;
        if n == 0.0 {
            return Vec::new();
        }
        let avg_len = (self.total_length as f64) / n;
        let tokens = tokenize(query);
        if tokens.is_empty() {
            return Vec::new();
        }
        let mut scores: HashMap<String, f64> = HashMap::new();
        for tok in tokens.iter() {
            let Some(postings) = self.postings.get(tok) else {
                continue;
            };
            let df = postings.len() as f64;
            // BM25 idf with the +0.5 add-half smoothing.
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for (doc_id, tf) in postings {
                let dl = *self.doc_lengths.get(doc_id).unwrap_or(&0) as f64;
                let tf = *tf as f64;
                let norm = 1.0 - BM25_B + BM25_B * dl / avg_len.max(1.0);
                let s = idf * tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * norm);
                *scores.entry(doc_id.clone()).or_insert(0.0) += s;
            }
        }
        let mut entries: Vec<(String, f32)> =
            scores.into_iter().map(|(id, s)| (id, s as f32)).collect();
        entries.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        entries.truncate(k);
        entries
    }
}

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "he", "in", "is", "it",
    "its", "of", "on", "or", "that", "the", "to", "was", "were", "will", "with", "this", "these",
    "those",
];

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .filter(|s| !STOP_WORDS.contains(&s.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ranks_relevant_doc_higher() {
        let mut idx = FullTextIndex::for_designation(FullTextDesignation {
            label: "Doc".into(),
            property: "text".into(),
        });
        idx.upsert(
            "d1",
            "Rust is a systems programming language focused on safety.",
        );
        idx.upsert(
            "d2",
            "Python is a popular dynamic programming language for data science.",
        );
        idx.upsert("d3", "The graph database holds tenant snapshots.");

        let results = idx.search("rust programming", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "d1");
        // d2 should appear since it shares "programming"
        let ids: Vec<&str> = results.iter().map(|(i, _)| i.as_str()).collect();
        assert!(ids.contains(&"d2"));
    }

    #[test]
    fn remove_excludes_doc_from_future_searches() {
        let mut idx = FullTextIndex::for_designation(FullTextDesignation {
            label: "Doc".into(),
            property: "text".into(),
        });
        idx.upsert("d1", "alpha beta");
        idx.upsert("d2", "alpha");
        idx.remove("d1");
        let results = idx.search("alpha", 5);
        let ids: Vec<&str> = results.iter().map(|(i, _)| i.as_str()).collect();
        assert!(!ids.contains(&"d1"));
        assert!(ids.contains(&"d2"));
    }

    #[test]
    fn upsert_replaces_existing_text() {
        let mut idx = FullTextIndex::for_designation(FullTextDesignation {
            label: "Doc".into(),
            property: "text".into(),
        });
        idx.upsert("d1", "knowledge graph database");
        // overwrite
        idx.upsert("d1", "weather forecast for tomorrow");
        let r = idx.search("knowledge", 5);
        assert!(r.is_empty());
        let r = idx.search("weather", 5);
        assert_eq!(r[0].0, "d1");
    }

    // §P5-A pa5.1 + cc.3: backend trait + designation-required guarantees.

    #[test]
    fn fulltext_index_implements_fulltext_backend_trait() {
        let designation = FullTextDesignation {
            label: "Doc".into(),
            property: "text".into(),
        };
        let mut backend: Box<dyn FullTextBackend> =
            Box::new(FullTextIndex::for_designation(designation));
        backend.upsert("d1", "the quick brown fox");
        backend.upsert("d2", "the lazy dog");
        let hits = backend.search("fox", 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0, "d1");
        assert_eq!(backend.doc_count(), 2);
    }

    #[test]
    fn designation_is_owned_value_not_optional() {
        let idx = FullTextIndex::for_designation(FullTextDesignation {
            label: "Doc".into(),
            property: "text".into(),
        });
        assert_eq!(idx.designation.label, "Doc");
        assert_eq!(idx.designation.property, "text");
    }

    // §P5-A pa5.3: env-switch factory. Tests go through the pure dispatcher so
    // they can run in parallel without mutating the global env.

    fn fixture_designation() -> FullTextDesignation {
        FullTextDesignation {
            label: "Doc".into(),
            property: "text".into(),
        }
    }

    #[test]
    fn make_fulltext_backend_defaults_to_hand_rolled() {
        let backend = make_fulltext_backend_from_value(fixture_designation(), "").unwrap();
        assert_eq!(backend.designation().label, "Doc");
        assert_eq!(backend.doc_count(), 0);
    }

    #[test]
    fn make_fulltext_backend_rejects_unknown_backend() {
        let err = make_fulltext_backend_from_value(fixture_designation(), "elastic")
            .expect_err("unknown backend should error");
        assert_eq!(err.code(), "unknown_fulltext_backend");
    }

    #[cfg(not(feature = "tantivy"))]
    #[test]
    fn make_fulltext_backend_errors_when_tantivy_requested_without_feature() {
        let err = make_fulltext_backend_from_value(fixture_designation(), "tantivy")
            .expect_err("tantivy without feature should error");
        assert_eq!(err.code(), "unknown_fulltext_backend");
        assert!(err.message().to_ascii_lowercase().contains("tantivy"));
    }
}

// §P5-A pa5.1: the hand-rolled backend wired through the trait. The tantivy
// alternative lives in `crate::fulltext_tantivy` behind the `tantivy` feature.
impl FullTextBackend for FullTextIndex {
    fn upsert(&mut self, doc_id: &str, text: &str) {
        FullTextIndex::upsert(self, doc_id, text);
    }

    fn remove(&mut self, doc_id: &str) {
        FullTextIndex::remove(self, doc_id);
    }

    fn search(&self, query: &str, k: usize) -> Vec<(String, f32)> {
        FullTextIndex::search(self, query, k)
    }

    fn designation(&self) -> &FullTextDesignation {
        &self.designation
    }

    fn doc_count(&self) -> usize {
        FullTextIndex::doc_count(self)
    }
}

/// §P5-A pa5.3: env-switch factory. Reads `RUSTY_RED_FULLTEXT_BACKEND` and
/// forwards to the pure dispatcher below. The pure dispatcher exists so unit
/// tests can exercise every code path without racing on a process-global env
/// var; this thin wrapper is the production entry point.
pub fn make_fulltext_backend(
    designation: FullTextDesignation,
) -> Result<Box<dyn FullTextBackend>, FullTextBackendError> {
    let raw = std::env::var(RUSTY_RED_FULLTEXT_BACKEND_ENV).unwrap_or_default();
    make_fulltext_backend_from_value(designation, &raw)
}

/// Pure dispatcher; takes the env-var value as an explicit parameter so unit
/// tests can run in parallel without mutating global state. Default ("" /
/// "hand_rolled" / "hand-rolled" / "bm25") returns the BM25 impl. `"tantivy"`
/// returns the tantivy-backed impl when compiled with `--features tantivy`;
/// otherwise an explicit error so the caller knows to rebuild.
pub fn make_fulltext_backend_from_value(
    designation: FullTextDesignation,
    raw: &str,
) -> Result<Box<dyn FullTextBackend>, FullTextBackendError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | FULLTEXT_BACKEND_HAND_ROLLED | "hand-rolled" | "bm25" => {
            Ok(Box::new(FullTextIndex::for_designation(designation)))
        }
        FULLTEXT_BACKEND_TANTIVY => {
            #[cfg(feature = "tantivy")]
            {
                Ok(Box::new(
                    crate::fulltext_tantivy::TantivyFullTextBackend::new(designation)
                        .map_err(FullTextBackendError::TantivyInit)?,
                ))
            }
            #[cfg(not(feature = "tantivy"))]
            {
                let _ = designation;
                Err(FullTextBackendError::UnknownBackend(
                    "tantivy backend requires building with --features tantivy".to_string(),
                ))
            }
        }
        other => Err(FullTextBackendError::UnknownBackend(format!(
            "unknown {RUSTY_RED_FULLTEXT_BACKEND_ENV} value: {other}"
        ))),
    }
}

#[derive(Debug, Clone)]
pub enum FullTextBackendError {
    UnknownBackend(String),
    TantivyInit(String),
}

impl FullTextBackendError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownBackend(_) => "unknown_fulltext_backend",
            Self::TantivyInit(_) => "tantivy_init_failed",
        }
    }
    pub fn message(&self) -> String {
        match self {
            Self::UnknownBackend(s) => s.clone(),
            Self::TantivyInit(s) => format!("tantivy initialization failed: {s}"),
        }
    }
}

impl std::fmt::Display for FullTextBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for FullTextBackendError {}
