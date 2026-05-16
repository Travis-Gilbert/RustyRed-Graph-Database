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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FullTextDesignation {
    pub label: String,
    pub property: String,
}

#[derive(Debug, Default)]
pub struct FullTextIndex {
    pub designation: Option<FullTextDesignation>,
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

impl FullTextIndex {
    pub fn for_designation(d: FullTextDesignation) -> Self {
        Self {
            designation: Some(d),
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
}
