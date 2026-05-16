//! Phase 7 observability surface: atomic counters, slow-query ring buffer,
//! and a Prometheus-text-format /metrics renderer.
//!
//! Designed to be zero-overhead on the hot path: every counter is a
//! lock-free `AtomicU64::fetch_add`, the slow-query buffer is a fixed-size
//! mutex-guarded vec, and the Prometheus output is produced once per
//! scrape from a snapshot of the counters.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct CounterSet {
    total_requests: AtomicU64,
    errors: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cache_stale: AtomicU64,
    vector_search_calls: AtomicU64,
    fulltext_search_calls: AtomicU64,
    ppr_calls: AtomicU64,
    pagerank_calls: AtomicU64,
    components_calls: AtomicU64,
    communities_calls: AtomicU64,
    spatial_search_calls: AtomicU64,
    graph_mutations: AtomicU64,
    cypher_queries: AtomicU64,
    transactions_begun: AtomicU64,
    transactions_committed: AtomicU64,
    transactions_rolled_back: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct SlowQuery {
    pub recorded_at_unix_ms: u128,
    pub nanos: u64,
    pub kind: String,
    pub detail: String,
    pub nodes_visited: u64,
    pub edges_touched: u64,
}

#[derive(Clone)]
pub struct Observability {
    counters: Arc<CounterSet>,
    slow_queries: Arc<Mutex<Vec<SlowQuery>>>,
    slow_query_threshold_nanos: u64,
    slow_query_capacity: usize,
}

impl Default for Observability {
    fn default() -> Self {
        Self::new(100_000_000, 128) // 100ms threshold, 128-entry ring
    }
}

impl Observability {
    pub fn new(slow_query_threshold_nanos: u64, slow_query_capacity: usize) -> Self {
        Self {
            counters: Arc::new(CounterSet::default()),
            slow_queries: Arc::new(Mutex::new(Vec::with_capacity(slow_query_capacity))),
            slow_query_threshold_nanos,
            slow_query_capacity,
        }
    }

    // ---- counter increments (always cheap, no allocation) -----

    pub fn record_request(&self) {
        self.counters.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.counters.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cache_hit(&self) {
        self.counters.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cache_miss(&self) {
        self.counters.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cache_stale(&self) {
        self.counters.cache_stale.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_vector_search(&self) {
        self.counters
            .vector_search_calls
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fulltext_search(&self) {
        self.counters
            .fulltext_search_calls
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_ppr(&self) {
        self.counters.ppr_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_pagerank(&self) {
        self.counters.pagerank_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_components(&self) {
        self.counters
            .components_calls
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_communities(&self) {
        self.counters
            .communities_calls
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_spatial_search(&self) {
        self.counters
            .spatial_search_calls
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_mutation(&self) {
        self.counters.graph_mutations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cypher(&self) {
        self.counters.cypher_queries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_transaction_begin(&self) {
        self.counters
            .transactions_begun
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_transaction_commit(&self) {
        self.counters
            .transactions_committed
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_transaction_rollback(&self) {
        self.counters
            .transactions_rolled_back
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record a query that exceeded the slow threshold. Allocates only when
    /// the threshold is exceeded.
    pub fn record_query_timing(
        &self,
        kind: &str,
        detail: &str,
        nanos: u64,
        nodes_visited: u64,
        edges_touched: u64,
    ) {
        if nanos < self.slow_query_threshold_nanos {
            return;
        }
        let entry = SlowQuery {
            recorded_at_unix_ms: unix_ms(),
            nanos,
            kind: kind.to_string(),
            detail: detail.chars().take(256).collect(),
            nodes_visited,
            edges_touched,
        };
        let Ok(mut buf) = self.slow_queries.lock() else {
            return;
        };
        if buf.len() >= self.slow_query_capacity {
            buf.remove(0);
        }
        buf.push(entry);
    }

    pub fn snapshot_slow_queries(&self) -> Vec<SlowQuery> {
        self.slow_queries
            .lock()
            .map(|q| q.clone())
            .unwrap_or_default()
    }

    /// Render counters in Prometheus text format. Stable label set, no
    /// dynamic labels (per-tenant labels would explode cardinality).
    pub fn render_prometheus(&self) -> String {
        let c = &self.counters;
        let mut out = String::with_capacity(2048);
        write_counter(
            &mut out,
            "thg_total_requests",
            "Total HTTP requests received",
            c.total_requests.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_errors",
            "Total HTTP requests that returned an error status",
            c.errors.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_cache_hits",
            "GraphCache hits",
            c.cache_hits.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_cache_misses",
            "GraphCache misses",
            c.cache_misses.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_cache_stale",
            "GraphCache stale-on-graph-version hits",
            c.cache_stale.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_vector_search_calls",
            "Vector search calls",
            c.vector_search_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_fulltext_search_calls",
            "Full-text search calls",
            c.fulltext_search_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_ppr_calls",
            "Personalized PageRank calls",
            c.ppr_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_pagerank_calls",
            "Global PageRank calls",
            c.pagerank_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_components_calls",
            "Connected-components calls",
            c.components_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_communities_calls",
            "Community-detection calls",
            c.communities_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_spatial_search_calls",
            "Spatial radius/bbox search calls",
            c.spatial_search_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_graph_mutations",
            "Graph mutations (node/edge upserts and deletes)",
            c.graph_mutations.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_cypher_queries",
            "Cypher queries executed",
            c.cypher_queries.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_transactions_begun",
            "Transactions begun",
            c.transactions_begun.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_transactions_committed",
            "Transactions committed",
            c.transactions_committed.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "thg_transactions_rolled_back",
            "Transactions rolled back",
            c.transactions_rolled_back.load(Ordering::Relaxed),
        );
        out
    }
}

fn write_counter(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(" counter\n");
    out.push_str(name);
    out.push(' ');
    out.push_str(&value.to_string());
    out.push('\n');
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment_and_render() {
        let obs = Observability::default();
        obs.record_request();
        obs.record_request();
        obs.record_cache_hit();
        obs.record_vector_search();
        obs.record_ppr();

        let prom = obs.render_prometheus();
        assert!(prom.contains("thg_total_requests 2"));
        assert!(prom.contains("thg_cache_hits 1"));
        assert!(prom.contains("thg_vector_search_calls 1"));
        assert!(prom.contains("thg_ppr_calls 1"));
        assert!(prom.contains("# TYPE thg_total_requests counter"));
    }

    #[test]
    fn slow_query_ring_buffer_bounded() {
        let obs = Observability::new(0, 3); // record everything, cap 3
        for i in 0..10 {
            obs.record_query_timing("cypher", &format!("q{i}"), 1_000_000, i, i * 2);
        }
        let entries = obs.snapshot_slow_queries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].detail, "q7");
        assert_eq!(entries[2].detail, "q9");
    }

    #[test]
    fn slow_query_threshold_excludes_fast_queries() {
        let obs = Observability::new(100, 16);
        obs.record_query_timing("cypher", "fast", 50, 0, 0);
        obs.record_query_timing("cypher", "slow", 200, 1, 1);
        let entries = obs.snapshot_slow_queries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].detail, "slow");
    }
}
