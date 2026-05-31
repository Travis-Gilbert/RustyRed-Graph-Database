//! Substrate-native local graph search — the READ seam.
//!
//! The browser/crawler WRITE side (`ingest_loaded_pages` / `build_v2_fixture_crawl`)
//! turns every loaded page into graph state: `Page` + `ContentSnapshot` + `Domain`
//! nodes joined by `LINKS_TO` / `HAS_SNAPSHOT` / `ON_DOMAIN`. This module is the
//! complementary READ side, the consumer-side seam the substrate-native browser's
//! home surface, the crawler dashboard, and the harness all call:
//!
//!   given the substrate those writes accumulate in + a query,
//!   return the relevant NEIGHBOURHOOD of the page graph.
//!
//! It does not return a flat ranked list. It returns the *shape of what the user
//! has browsed about the query*: the pages that match (`ring 0`) plus the pages
//! reachable from them through real `LINKS_TO` edges (`ring 1..max_ring`). A topic
//! the user has explored deeply comes back as a dense, connected subgraph; a fresh
//! topic comes back as a lone page or nothing at all (the honest sparse case).
//!
//! Why a seam, not an engine: this is pure-Rust and Servo-free, so it compiles and
//! tests in seconds (the whole point of keeping the valuable logic out of the
//! ~30-minute Servo build), and it reads the substrate through the engine-agnostic
//! `GraphStore` trait (`query_nodes` + `neighbors`), so it works identically over
//! the in-memory store, a tenant-scoped store, or any future backing.
//!
//! On-spec per `docs/plans/rusty-red-web/implementation-plan.md` ("local graph
//! search", V0). Mirrors the browser-side relevance model so the crawler,
//! search page, and server route share one mental model.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use rustyred_core::graph_store::{GraphStore, NeighborQuery, NodeQuery, NodeRecord};
use rustyred_core::personalized_pagerank;
use serde::{Deserialize, Serialize};
use turbovec::IdMapIndex;
use url::Url;

use crate::{EDGE_HAS_SNAPSHOT, EDGE_LINKS_TO, LABEL_PAGE};

/// How many `LINKS_TO` hops out from a direct match to pull into the result.
const DEFAULT_MAX_RING: usize = 2;
/// Upper bound on how many `Page` nodes to scan from the substrate per search.
const DEFAULT_SCAN_LIMIT: usize = 10_000;
/// Snippet length (characters of the page's extracted text).
const SNIPPET_CHARS: usize = 240;
/// ACL local-push PageRank alpha, matching RustyRed's native graph algorithm default.
const DEFAULT_PPR_ALPHA: f64 = 0.15;
/// PPR residual threshold. Kept low enough for browser-size page graphs.
const DEFAULT_PPR_EPSILON: f64 = 1e-5;
/// Hard cap for local-push work. Browser substrates should stay well below this.
const DEFAULT_PPR_MAX_PUSHES: usize = 100_000;
/// Dense candidates to retrieve from the local TurboVec index before graph fusion.
const DEFAULT_DENSE_LIMIT: usize = 64;
/// Reciprocal rank fusion constant; 60 is the common IR default.
const DEFAULT_RRF_K: usize = 60;
/// Hash embedding dimensionality for the standalone dense layer.
const DENSE_DIM: usize = 128;
/// TurboVec quantization bit width for ephemeral local search indexes.
const DENSE_BIT_WIDTH: usize = 4;
/// Avoid arbitrary dense-only matches when the hash vector has no meaningful overlap.
const MIN_DENSE_SCORE: f32 = 0.08;

/// Knobs for a substrate search. `Default` is the common browser case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchOptions {
    /// Hops out from a direct match to include. 0 = matches only.
    pub max_ring: usize,
    /// Cap on `Page` nodes scanned. Protects against unbounded substrates.
    pub scan_limit: usize,
    /// Cap on dense vector candidates before graph expansion.
    pub dense_limit: usize,
    /// Reciprocal-rank fusion constant for lexical + dense candidate ranks.
    pub rrf_k: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_ring: DEFAULT_MAX_RING,
            scan_limit: DEFAULT_SCAN_LIMIT,
            dense_limit: DEFAULT_DENSE_LIMIT,
            rrf_k: DEFAULT_RRF_K,
        }
    }
}

/// One page in the result neighbourhood, annotated with how it got there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// The substrate node id of the `Page`.
    pub node_id: String,
    /// The page url (falls back to `canonical_url`).
    pub url: String,
    /// A human title derived from the url (last path segment or host).
    pub title: String,
    /// An excerpt of the page's extracted text (empty for a discovered-but-
    /// unfetched link target, which has no `ContentSnapshot` yet).
    pub snippet: String,
    /// Hop distance to the nearest direct match. 0 = the page itself matched.
    pub ring: usize,
    /// Plain-language ring name: match / adjacent / nearby / distant / browse.
    pub ring_label: String,
    /// Graph-aware relevance score. Direct matches seed native PPR; linked
    /// neighbours can receive non-zero score when the link graph supports them.
    pub match_score: f64,
}

/// A `LINKS_TO` edge that survives inside the result neighbourhood.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchLink {
    pub source: String,
    pub target: String,
}

/// The result of a substrate search: the matched pages + their link
/// neighbourhood, plus the edges among them. Serde-serializable so it can ride a
/// future `POST /v1/graph/query` / gRPC response unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubstrateSearch {
    /// The normalized query (trimmed, lower-cased). Empty = browse mode.
    pub query: String,
    /// Pages in the neighbourhood, ordered by (ring, node_id) for determinism.
    pub hits: Vec<SearchHit>,
    /// `LINKS_TO` edges whose both endpoints are in `hits`.
    pub links: Vec<SearchLink>,
    /// How many pages directly matched the query (ring 0).
    pub matched_count: usize,
    /// Total pages in the returned neighbourhood.
    pub kept_count: usize,
}

fn ring_label(ring: usize) -> &'static str {
    match ring {
        0 => "match",
        1 => "adjacent",
        2 => "nearby",
        _ => "distant",
    }
}

fn prop_str(node: &NodeRecord, key: &str) -> Option<String> {
    node.properties
        .get(key)
        .and_then(|value| value.as_str())
        .map(|s| s.to_string())
}

/// A readable title from a url: the last non-empty path segment, else the host,
/// else the raw url.
fn title_from_url(url: &str) -> String {
    if let Ok(parsed) = Url::parse(url) {
        if let Some(segment) = parsed
            .path_segments()
            .and_then(|segments| segments.filter(|s| !s.is_empty()).next_back())
        {
            return segment.to_string();
        }
        if let Some(host) = parsed.host_str() {
            return host.to_string();
        }
    }
    url.to_string()
}

/// Character-safe truncation to `SNIPPET_CHARS`.
fn snippet_of(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= SNIPPET_CHARS {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(SNIPPET_CHARS).collect();
    out.push('\u{2026}'); // ellipsis
    out
}

/// Lexical relevance of a page against the query terms. A url/title hit weighs
/// double a body hit: a page titled for the query is more relevant than one that
/// merely mentions it. Returns 0 for no match.
fn score_page(url: &str, text: &str, terms: &[String]) -> u32 {
    let url_l = url.to_lowercase();
    let text_l = text.to_lowercase();
    let mut score = 0u32;
    for term in terms {
        if url_l.contains(term.as_str()) {
            score += 2;
        }
        if text_l.contains(term.as_str()) {
            score += 1;
        }
    }
    score
}

/// The extracted text of a page, read from its `ContentSnapshot` via `HAS_SNAPSHOT`.
/// Empty when the page has no snapshot (a discovered link target never fetched).
fn snapshot_text(store: &impl GraphStore, page_id: &str) -> String {
    let mut hits = store.neighbors(NeighborQuery::out(page_id).with_edge_type(EDGE_HAS_SNAPSHOT));
    hits.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    for hit in hits {
        if let Some(node) = store.get_node(&hit.node_id) {
            if let Some(text) = prop_str(node, "text") {
                return text;
            }
        }
    }
    String::new()
}

/// The `LINKS_TO` neighbours of a page in BOTH directions (a page you linked from
/// and a page that links to it are equally "near" in your browsing), sorted for
/// deterministic BFS.
fn linked_neighbours(store: &impl GraphStore, page_id: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for hit in store.neighbors(NeighborQuery::out(page_id).with_edge_type(EDGE_LINKS_TO)) {
        out.insert(hit.node_id);
    }
    for hit in store.neighbors(NeighborQuery::in_(page_id).with_edge_type(EDGE_LINKS_TO)) {
        out.insert(hit.node_id);
    }
    out.into_iter().collect()
}

/// url + title + snippet for a page node id, fetching the node if needed.
fn page_meta(store: &impl GraphStore, page_id: &str) -> (String, String, String) {
    let url = store
        .get_node(page_id)
        .and_then(|node| prop_str(node, "url").or_else(|| prop_str(node, "canonical_url")))
        .unwrap_or_default();
    let title = title_from_url(&url);
    let snippet = snippet_of(&snapshot_text(store, page_id));
    (url, title, snippet)
}

/// The `LINKS_TO` edges whose source and target are both in `kept`.
fn links_among(store: &impl GraphStore, kept: &BTreeSet<String>) -> Vec<SearchLink> {
    let mut links: BTreeSet<(String, String)> = BTreeSet::new();
    for source in kept {
        for hit in store.neighbors(NeighborQuery::out(source).with_edge_type(EDGE_LINKS_TO)) {
            if kept.contains(&hit.node_id) {
                links.insert((source.clone(), hit.node_id));
            }
        }
    }
    links
        .into_iter()
        .map(|(source, target)| SearchLink { source, target })
        .collect()
}

fn ppr_adjacency(
    store: &impl GraphStore,
    pages: &[NodeRecord],
) -> HashMap<String, Vec<(String, f64)>> {
    let known: BTreeSet<String> = pages.iter().map(|page| page.id.clone()).collect();
    let mut adjacency: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for page in pages {
        adjacency.entry(page.id.clone()).or_default();
        for hit in store
            .neighbors(NeighborQuery::out(&page.id).with_edge_type(EDGE_LINKS_TO))
            .into_iter()
            .filter(|hit| known.contains(&hit.node_id))
        {
            adjacency
                .entry(page.id.clone())
                .or_default()
                .entry(hit.node_id.clone())
                .or_insert(1.0);
            adjacency
                .entry(hit.node_id)
                .or_default()
                .entry(page.id.clone())
                .or_insert(1.0);
        }
    }
    adjacency
        .into_iter()
        .map(|(source, targets)| (source, targets.into_iter().collect()))
        .collect()
}

fn ppr_seed_scores_f64(score_of: &BTreeMap<String, f64>) -> HashMap<String, f64> {
    let total: f64 = score_of.values().sum();
    if total <= 0.0 {
        return HashMap::new();
    }
    score_of
        .iter()
        .map(|(id, score)| (id.clone(), *score / total))
        .collect()
}

fn rank_score(
    id: &str,
    lexical_scores: &BTreeMap<String, f64>,
    ppr_scores: &HashMap<String, f64>,
) -> f64 {
    ppr_scores
        .get(id)
        .copied()
        .or_else(|| lexical_scores.get(id).copied())
        .unwrap_or(0.0)
}

fn compare_ranked_hits(a: &(String, usize, f64), b: &(String, usize, f64)) -> Ordering {
    b.2.partial_cmp(&a.2)
        .unwrap_or(Ordering::Equal)
        .then_with(|| a.1.cmp(&b.1))
        .then_with(|| a.0.cmp(&b.0))
}

fn stable_dense_id(page_id: &str, used: &mut BTreeSet<u64>) -> u64 {
    let digest = blake3::hash(page_id.as_bytes());
    let bytes = digest.as_bytes();
    let mut id = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    while !used.insert(id) {
        id = id.wrapping_add(1);
    }
    id
}

fn normalized_features(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 2)
        .map(str::to_string)
        .collect()
}

fn add_hash_feature(vector: &mut [f32], feature: &str, weight: f32) {
    let digest = blake3::hash(feature.as_bytes());
    let bytes = digest.as_bytes();
    let bucket = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize % DENSE_DIM;
    let sign = if bytes[4] & 1 == 0 { 1.0 } else { -1.0 };
    vector[bucket] += sign * weight;
}

fn hash_embed_text(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0; DENSE_DIM];
    let features = normalized_features(text);
    for token in &features {
        add_hash_feature(&mut vector, token, 1.0);
        if token.len() >= 3 {
            let chars: Vec<char> = token.chars().collect();
            for window in chars.windows(3) {
                let trigram: String = window.iter().collect();
                add_hash_feature(&mut vector, &format!("tri:{trigram}"), 0.35);
            }
        }
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 1e-6 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn dense_candidate_scores(
    pages: &[NodeRecord],
    meta: &BTreeMap<String, (String, String, String)>,
    query: &str,
    k: usize,
) -> BTreeMap<String, f64> {
    if k == 0 || pages.is_empty() {
        return BTreeMap::new();
    }
    let query_vector = hash_embed_text(query);
    if query_vector.iter().all(|value| value.abs() < 1e-6) {
        return BTreeMap::new();
    }

    let mut vectors = Vec::with_capacity(pages.len() * DENSE_DIM);
    let mut ids = Vec::with_capacity(pages.len());
    let mut id_to_page: HashMap<u64, String> = HashMap::with_capacity(pages.len());
    let mut used_ids = BTreeSet::new();
    for page in pages {
        let Some((url, title, text)) = meta.get(&page.id) else {
            continue;
        };
        let dense_text = format!("{url} {title} {text}");
        let vector = hash_embed_text(&dense_text);
        if vector.iter().all(|value| value.abs() < 1e-6) {
            continue;
        }
        let dense_id = stable_dense_id(&page.id, &mut used_ids);
        id_to_page.insert(dense_id, page.id.clone());
        ids.push(dense_id);
        vectors.extend(vector);
    }
    if ids.is_empty() {
        return BTreeMap::new();
    }

    let mut index = match IdMapIndex::new(DENSE_DIM, DENSE_BIT_WIDTH) {
        Ok(index) => index,
        Err(_) => return BTreeMap::new(),
    };
    if index.add_with_ids(&vectors, &ids).is_err() {
        return BTreeMap::new();
    }
    let (scores, result_ids) = index.search(&query_vector, k.min(ids.len()));
    scores
        .into_iter()
        .zip(result_ids)
        .filter_map(|(score, dense_id)| {
            if score < MIN_DENSE_SCORE {
                return None;
            }
            id_to_page
                .get(&dense_id)
                .map(|page_id| (page_id.clone(), score as f64))
        })
        .collect()
}

fn add_rrf_scores(
    fused_scores: &mut BTreeMap<String, f64>,
    mut ranked: Vec<(String, f64)>,
    rrf_k: usize,
) {
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    for (rank, (id, _score)) in ranked.into_iter().enumerate() {
        let contribution = 1.0 / (rrf_k.saturating_add(rank + 1) as f64);
        *fused_scores.entry(id).or_insert(0.0) += contribution;
    }
}

/// Search the substrate for the shape of the user's knowledge about `query`.
///
/// Empty query => browse mode: every `Page` node (bounded by `scan_limit`),
/// unranked. Otherwise: pages whose url or extracted text match the query terms
/// (`ring 0`) seed native RustyRed PPR over the page-link graph, then the result is
/// expanded out along `LINKS_TO` to `max_ring` hops. Ring labels explain how a
/// page entered the neighbourhood; score and ordering come from graph rank.
pub fn search_substrate(
    store: &impl GraphStore,
    query: &str,
    options: SearchOptions,
) -> SubstrateSearch {
    let normalized = query.trim().to_lowercase();

    // Scan Page nodes in a deterministic order.
    let mut pages = store.query_nodes(NodeQuery::label(LABEL_PAGE).with_limit(options.scan_limit));
    pages.sort_by(|a, b| a.id.cmp(&b.id));

    // Per-page meta (url / title / snippet), read once.
    let mut meta: BTreeMap<String, (String, String, String)> = BTreeMap::new();
    for page in &pages {
        let url = prop_str(page, "url")
            .or_else(|| prop_str(page, "canonical_url"))
            .unwrap_or_default();
        let title = title_from_url(&url);
        let snippet = snapshot_text(store, &page.id);
        meta.insert(page.id.clone(), (url, title, snippet));
    }

    // Browse mode: the whole page graph, unranked.
    if normalized.is_empty() {
        let hits: Vec<SearchHit> = pages
            .iter()
            .map(|page| {
                let (url, title, snippet) = meta.get(&page.id).cloned().unwrap_or_default();
                SearchHit {
                    node_id: page.id.clone(),
                    url,
                    title,
                    snippet: snippet_of(&snippet),
                    ring: 0,
                    ring_label: "browse".to_string(),
                    match_score: 0.0,
                }
            })
            .collect();
        let kept: BTreeSet<String> = pages.iter().map(|p| p.id.clone()).collect();
        let links = links_among(store, &kept);
        return SubstrateSearch {
            query: String::new(),
            kept_count: hits.len(),
            matched_count: 0,
            hits,
            links,
        };
    }

    let terms: Vec<String> = normalized
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    // Ring 0: direct lexical and dense matches.
    let mut lexical_score_of: BTreeMap<String, u32> = BTreeMap::new();
    for page in &pages {
        if let Some((url, _title, text)) = meta.get(&page.id) {
            let score = score_page(url, text, &terms);
            if score > 0 {
                lexical_score_of.insert(page.id.clone(), score);
            }
        }
    }
    let dense_score_of = dense_candidate_scores(&pages, &meta, &normalized, options.dense_limit);
    let mut score_of: BTreeMap<String, f64> = BTreeMap::new();
    add_rrf_scores(
        &mut score_of,
        lexical_score_of
            .iter()
            .map(|(id, score)| (id.clone(), *score as f64))
            .collect(),
        options.rrf_k,
    );
    add_rrf_scores(
        &mut score_of,
        dense_score_of.into_iter().collect(),
        options.rrf_k,
    );

    // BFS out along LINKS_TO. ring = minimum hop distance to any match.
    let mut ring_of: BTreeMap<String, usize> = BTreeMap::new();
    for id in score_of.keys() {
        ring_of.insert(id.clone(), 0);
    }
    let mut frontier: Vec<String> = score_of.keys().cloned().collect(); // BTreeMap => sorted
    for ring in 1..=options.max_ring {
        if frontier.is_empty() {
            break;
        }
        let mut next: BTreeSet<String> = BTreeSet::new();
        for id in &frontier {
            for neighbour in linked_neighbours(store, id) {
                if !ring_of.contains_key(&neighbour) {
                    ring_of.insert(neighbour.clone(), ring);
                    next.insert(neighbour);
                }
            }
        }
        frontier = next.into_iter().collect();
    }

    let ppr_scores = personalized_pagerank(
        &ppr_adjacency(store, &pages),
        &ppr_seed_scores_f64(&score_of),
        DEFAULT_PPR_ALPHA,
        DEFAULT_PPR_EPSILON,
        DEFAULT_PPR_MAX_PUSHES,
    );

    // Emit hits ordered by graph-aware rank, then ring, then node id.
    let kept_ids: BTreeSet<String> = ring_of.keys().cloned().collect();
    let mut ordered: Vec<(String, usize, f64)> = ring_of
        .into_iter()
        .map(|(id, ring)| {
            let score = rank_score(&id, &score_of, &ppr_scores);
            (id, ring, score)
        })
        .collect();
    ordered.sort_by(compare_ranked_hits);

    let hits: Vec<SearchHit> = ordered
        .into_iter()
        .map(|(id, ring, score)| {
            let (url, title, snippet) = meta
                .get(&id)
                .cloned()
                .unwrap_or_else(|| page_meta(store, &id));
            SearchHit {
                match_score: score,
                node_id: id,
                url,
                title,
                snippet: snippet_of(&snippet),
                ring,
                ring_label: ring_label(ring).to_string(),
            }
        })
        .collect();

    let links = links_among(store, &kept_ids);
    SubstrateSearch {
        matched_count: score_of.len(),
        kept_count: hits.len(),
        query: normalized,
        hits,
        links,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_v2_fixture_crawl, CrawlRequest, FetchedPage};
    use rustyred_core::graph_store::InMemoryGraphStore;

    fn page(url: &str, body: &str) -> FetchedPage {
        FetchedPage {
            url: url.to_string(),
            status: 200,
            body: body.to_string(),
            content_type: "text/html; charset=utf-8".to_string(),
            fetched_at: String::new(),
        }
    }

    /// A small browsed substrate:
    ///   apple --LINKS_TO--> orchard --LINKS_TO--> soil
    ///   banana (unrelated, unlinked)
    /// Only the apple page mentions "apple"; orchard/soil/banana do not.
    fn browsed_store() -> InMemoryGraphStore {
        let pages = vec![
            page(
                "http://ex.com/apple",
                r#"<html><body>apple cultivation and apple varieties
                   <a href="/orchard">orchard</a></body></html>"#,
            ),
            page(
                "http://ex.com/orchard",
                r#"<html><body>orchard management and trees
                   <a href="/soil">soil</a></body></html>"#,
            ),
            page(
                "http://ex.com/soil",
                r#"<html><body>soil composition and drainage</body></html>"#,
            ),
            page(
                "http://ex.com/banana",
                r#"<html><body>bananas are yellow</body></html>"#,
            ),
        ];
        let seeds = pages.iter().map(|p| p.url.clone()).collect();
        let output = build_v2_fixture_crawl(CrawlRequest::new("search-test", seeds), &pages)
            .expect("fixture crawl should build");
        let mut store = InMemoryGraphStore::new();
        output
            .graph
            .apply_to_store(&mut store)
            .expect("apply to store should succeed");
        store
    }

    fn urls(search: &SubstrateSearch) -> Vec<String> {
        search.hits.iter().map(|h| h.url.clone()).collect()
    }

    #[test]
    fn ring_0_is_the_direct_match_only() {
        let store = browsed_store();
        let out = search_substrate(
            &store,
            "apple",
            SearchOptions {
                max_ring: 0,
                ..SearchOptions::default()
            },
        );
        assert_eq!(out.matched_count, 1, "only the apple page mentions apple");
        assert_eq!(out.kept_count, 1, "max_ring 0 keeps matches only");
        let hit = &out.hits[0];
        assert_eq!(hit.url, "http://ex.com/apple");
        assert_eq!(hit.ring, 0);
        assert_eq!(hit.ring_label, "match");
        assert!(hit.match_score > 0.0);
        assert!(hit.snippet.contains("apple"), "snippet carries the text");
    }

    #[test]
    fn ring_1_pulls_in_the_linked_neighbour() {
        let store = browsed_store();
        let out = search_substrate(
            &store,
            "apple",
            SearchOptions {
                max_ring: 1,
                ..SearchOptions::default()
            },
        );
        // apple (ring 0) + orchard (ring 1, linked from apple). soil is 2 hops
        // out; banana is unrelated and unlinked. Both excluded at max_ring 1.
        let kept = urls(&out);
        assert!(kept.contains(&"http://ex.com/apple".to_string()));
        assert!(kept.contains(&"http://ex.com/orchard".to_string()));
        assert!(
            !kept.contains(&"http://ex.com/soil".to_string()),
            "soil is 2 hops"
        );
        assert!(
            !kept.contains(&"http://ex.com/banana".to_string()),
            "banana unrelated"
        );

        let orchard = out
            .hits
            .iter()
            .find(|h| h.url == "http://ex.com/orchard")
            .unwrap();
        assert_eq!(orchard.ring, 1);
        assert!(
            orchard.match_score > 0.0,
            "linked neighbours receive graph-rank mass"
        );

        // The surviving LINKS_TO edge (apple -> orchard) is reported.
        assert!(
            out.links
                .iter()
                .any(|l| store_url(&store, &l.source) == "http://ex.com/apple"
                    && store_url(&store, &l.target) == "http://ex.com/orchard"),
            "apple->orchard link is in the neighbourhood"
        );
    }

    #[test]
    fn ring_2_reaches_the_second_hop() {
        let store = browsed_store();
        let out = search_substrate(
            &store,
            "apple",
            SearchOptions {
                max_ring: 2,
                ..SearchOptions::default()
            },
        );
        let kept = urls(&out);
        assert!(
            kept.contains(&"http://ex.com/soil".to_string()),
            "soil reached at ring 2"
        );
        let soil = out
            .hits
            .iter()
            .find(|h| h.url == "http://ex.com/soil")
            .unwrap();
        assert_eq!(soil.ring, 2);
        assert_eq!(soil.ring_label, "nearby");
        assert!(soil.match_score > 0.0, "PPR reaches the second hop");
    }

    #[test]
    fn no_match_is_the_honest_sparse_case() {
        let store = browsed_store();
        let out = search_substrate(&store, "quantum chromodynamics", SearchOptions::default());
        assert_eq!(out.matched_count, 0);
        assert_eq!(out.kept_count, 0);
        assert!(out.hits.is_empty());
        assert!(out.links.is_empty());
    }

    #[test]
    fn empty_query_is_browse_mode() {
        let store = browsed_store();
        let out = search_substrate(&store, "   ", SearchOptions::default());
        assert_eq!(out.query, "");
        assert_eq!(out.matched_count, 0);
        let kept = urls(&out);
        // All four fetched pages are present in browse mode.
        for url in [
            "http://ex.com/apple",
            "http://ex.com/orchard",
            "http://ex.com/soil",
            "http://ex.com/banana",
        ] {
            assert!(kept.contains(&url.to_string()), "browse mode shows {url}");
        }
    }

    #[test]
    fn is_deterministic() {
        let store = browsed_store();
        let first = search_substrate(&store, "apple", SearchOptions::default());
        let second = search_substrate(&store, "apple", SearchOptions::default());
        assert_eq!(first, second, "same substrate + query => identical result");
    }

    fn store_url(store: &InMemoryGraphStore, node_id: &str) -> String {
        store
            .get_node(node_id)
            .and_then(|n| prop_str(n, "url").or_else(|| prop_str(n, "canonical_url")))
            .unwrap_or_default()
    }
}
