//! Phase 7 observability surface: atomic counters, latency summaries,
//! slow-query ring buffer, optional JSONL slow-query export, and a
//! Prometheus-text-format /metrics renderer.
//!
//! Designed to be zero-overhead on the hot path: every counter is a
//! lock-free `AtomicU64::fetch_add`, the slow-query buffer is a fixed-size
//! mutex-guarded vec, and the Prometheus output is produced once per
//! scrape from a snapshot of the counters.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustyred_core::unix_ms;

// §P7-A pa7.1: latency-histogram constants and helpers.

/// Prometheus default latency buckets (in seconds). The default set covers
/// the typical web-service latency band; we exclude infinity here and emit
/// `+Inf` explicitly when rendering.
pub const LATENCY_BUCKETS_SECONDS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

// §cc.2: kind-name constants. Replace literal "cypher" / "algorithm:*" strings
// at the record-query-timing call sites so renaming flows through grep, not
// through string duplication.
pub const KIND_CYPHER: &str = "cypher";
pub const KIND_VECTOR_SEARCH: &str = "vector_search";
pub const KIND_FULLTEXT_SEARCH: &str = "fulltext_search";
pub const KIND_ALGO_PPR: &str = "algorithm:ppr";
pub const KIND_ALGO_PAGERANK: &str = "algorithm:pagerank";
pub const KIND_ALGO_COMPONENTS: &str = "algorithm:components";
pub const KIND_ALGO_COMMUNITIES: &str = "algorithm:communities";

/// Lock-free latency histogram. Bucket counters are cumulative (Prometheus
/// semantics): each `_bucket{le="X"}` value is the count of observations
/// whose value was less-than-or-equal to X.
#[derive(Debug)]
pub struct Histogram {
    bucket_boundaries: &'static [f64],
    bucket_counters: Vec<AtomicU64>,
    sum_nanos: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    pub fn new(bucket_boundaries: &'static [f64]) -> Self {
        // +1 slot for the +Inf overflow bucket.
        let mut bucket_counters = Vec::with_capacity(bucket_boundaries.len() + 1);
        for _ in 0..=bucket_boundaries.len() {
            bucket_counters.push(AtomicU64::new(0));
        }
        Self {
            bucket_boundaries,
            bucket_counters,
            sum_nanos: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    pub fn observe_nanos(&self, nanos: u64) {
        self.sum_nanos.fetch_add(nanos, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        let seconds = nanos as f64 / 1_000_000_000.0;
        let bucket = self
            .bucket_boundaries
            .iter()
            .position(|le| seconds <= *le)
            .unwrap_or(self.bucket_boundaries.len());
        for idx in bucket..self.bucket_counters.len() {
            self.bucket_counters[idx].fetch_add(1, Ordering::Relaxed);
        }
    }

    // Retained as part of the Histogram metric API surface.
    #[allow(dead_code)]
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn sum_nanos(&self) -> u64 {
        self.sum_nanos.load(Ordering::Relaxed)
    }

    /// Append Prometheus exposition lines for this histogram.
    pub fn render(&self, out: &mut String, name: &str, help: &str) {
        out.push_str("# HELP ");
        out.push_str(name);
        out.push(' ');
        out.push_str(help);
        out.push('\n');
        out.push_str("# TYPE ");
        out.push_str(name);
        out.push_str(" histogram\n");
        for (idx, le) in self.bucket_boundaries.iter().enumerate() {
            let count = self.bucket_counters[idx].load(Ordering::Relaxed);
            out.push_str(name);
            out.push_str("_bucket{le=\"");
            out.push_str(&format_bucket_bound(*le));
            out.push_str("\"} ");
            out.push_str(&count.to_string());
            out.push('\n');
        }
        let inf_count = self
            .bucket_counters
            .last()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0);
        out.push_str(name);
        out.push_str("_bucket{le=\"+Inf\"} ");
        out.push_str(&inf_count.to_string());
        out.push('\n');
        let sum_seconds = self.sum_nanos.load(Ordering::Relaxed) as f64 / 1_000_000_000.0;
        out.push_str(name);
        out.push_str("_sum ");
        out.push_str(&format!("{sum_seconds:.9}"));
        out.push('\n');
        out.push_str(name);
        out.push_str("_count ");
        out.push_str(&self.count.load(Ordering::Relaxed).to_string());
        out.push('\n');
    }
}

fn format_bucket_bound(le: f64) -> String {
    // Format with minimal trailing zeroes; Prometheus accepts both "0.005" and
    // "5e-3" but readable scientific-notation is uncommon, so keep decimals.
    if (le - le.round()).abs() < f64::EPSILON {
        format!("{le:.0}")
    } else {
        let formatted = format!("{le:.3}");
        // Trim trailing zeroes but keep at least one fractional digit.
        let trimmed = formatted.trim_end_matches('0');
        let trimmed = trimmed.trim_end_matches('.');
        trimmed.to_string()
    }
}

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
    timings: Arc<Mutex<BTreeMap<String, TimingWindow>>>,
    /// §P7-A pa7.2: per-kind latency histograms. Kinds align with the timing
    /// labels so `record_latency_sample` can observe into both surfaces with
    /// one lookup.
    histograms: Arc<Mutex<BTreeMap<String, Arc<Histogram>>>>,
    slow_query_threshold_nanos: u64,
    slow_query_capacity: usize,
    slow_query_log: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct TimingWindow {
    samples: Vec<u64>,
}

impl Default for Observability {
    fn default() -> Self {
        Self::new(100_000_000, 128) // 100ms threshold, 128-entry ring
    }
}

impl Observability {
    pub fn new(slow_query_threshold_nanos: u64, slow_query_capacity: usize) -> Self {
        Self::new_with_log(slow_query_threshold_nanos, slow_query_capacity, None)
    }

    pub fn new_with_log(
        slow_query_threshold_nanos: u64,
        slow_query_capacity: usize,
        slow_query_log: Option<String>,
    ) -> Self {
        let slow_query_capacity = slow_query_capacity.max(1);
        Self {
            counters: Arc::new(CounterSet::default()),
            slow_queries: Arc::new(Mutex::new(Vec::with_capacity(slow_query_capacity))),
            timings: Arc::new(Mutex::new(BTreeMap::new())),
            histograms: Arc::new(Mutex::new(BTreeMap::new())),
            slow_query_threshold_nanos,
            slow_query_capacity,
            slow_query_log,
        }
    }

    // ---- counter increments (always cheap, no allocation) -----

    #[allow(dead_code)]
    pub fn record_request(&self) {
        self.counters.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.counters.errors.fetch_add(1, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub fn record_cache_hit(&self) {
        self.counters.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub fn record_cache_miss(&self) {
        self.counters.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    #[allow(dead_code)]
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
        self.counters
            .graph_mutations
            .fetch_add(1, Ordering::Relaxed);
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
        self.record_latency_sample(kind, nanos);
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
        self.emit_slow_query(&entry);
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

    fn record_latency_sample(&self, kind: &str, nanos: u64) {
        let truncated: String = kind.chars().take(64).collect();
        if let Ok(mut timings) = self.timings.lock() {
            let window = timings
                .entry(truncated.clone())
                .or_insert_with(TimingWindow::default);
            if window.samples.len() >= self.slow_query_capacity {
                window.samples.remove(0);
            }
            window.samples.push(nanos);
        }
        // §P7-A pa7.2: also observe into the per-kind histogram so /metrics
        // can expose P50/P95/P99 buckets alongside the slow-query samples.
        if let Ok(mut histograms) = self.histograms.lock() {
            let histogram = histograms
                .entry(truncated)
                .or_insert_with(|| Arc::new(Histogram::new(LATENCY_BUCKETS_SECONDS)));
            histogram.observe_nanos(nanos);
        }
    }

    fn emit_slow_query(&self, entry: &SlowQuery) {
        let Some(target) = self.slow_query_log.as_deref() else {
            return;
        };
        let line = serde_json::json!({
            "recorded_at_unix_ms": entry.recorded_at_unix_ms.to_string(),
            "nanos": entry.nanos,
            "kind": entry.kind,
            "detail": entry.detail,
            "nodes_visited": entry.nodes_visited,
            "edges_touched": entry.edges_touched,
        })
        .to_string();
        if target.eq_ignore_ascii_case("stderr") {
            eprintln!("{line}");
            return;
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(target) {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Render counters in Prometheus text format. Stable label set, no
    /// dynamic labels (per-tenant labels would explode cardinality).
    pub fn render_prometheus(&self) -> String {
        let c = &self.counters;
        let mut out = String::with_capacity(2048);
        write_counter(
            &mut out,
            "rustyred_total_requests",
            "Total HTTP requests received",
            c.total_requests.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_errors",
            "Total HTTP requests that returned an error status",
            c.errors.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_cache_hits",
            "GraphCache hits",
            c.cache_hits.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_cache_misses",
            "GraphCache misses",
            c.cache_misses.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_cache_stale",
            "GraphCache stale-on-graph-version hits",
            c.cache_stale.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_vector_search_calls",
            "Vector search calls",
            c.vector_search_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_fulltext_search_calls",
            "Full-text search calls",
            c.fulltext_search_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_ppr_calls",
            "Personalized PageRank calls",
            c.ppr_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_pagerank_calls",
            "Global PageRank calls",
            c.pagerank_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_components_calls",
            "Connected-components calls",
            c.components_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_communities_calls",
            "Community-detection calls",
            c.communities_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_spatial_search_calls",
            "Spatial radius/bbox search calls",
            c.spatial_search_calls.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_graph_mutations",
            "Graph mutations (node/edge upserts and deletes)",
            c.graph_mutations.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_cypher_queries",
            "Cypher queries executed",
            c.cypher_queries.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_transactions_begun",
            "Transactions begun",
            c.transactions_begun.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_transactions_committed",
            "Transactions committed",
            c.transactions_committed.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "rustyred_transactions_rolled_back",
            "Transactions rolled back",
            c.transactions_rolled_back.load(Ordering::Relaxed),
        );
        if let Ok(timings) = self.timings.lock() {
            for (kind, window) in timings.iter() {
                if window.samples.is_empty() {
                    continue;
                }
                let mut samples = window.samples.clone();
                samples.sort_unstable();
                let count = samples.len() as u64;
                write_gauge_labeled(
                    &mut out,
                    "rustyred_query_latency_count",
                    "Recorded query latency samples by bounded query kind",
                    kind,
                    count,
                );
                write_gauge_labeled(
                    &mut out,
                    "rustyred_query_latency_p50_nanos",
                    "Rolling p50 query latency in nanoseconds by bounded query kind",
                    kind,
                    percentile(&samples, 0.50),
                );
                write_gauge_labeled(
                    &mut out,
                    "rustyred_query_latency_p95_nanos",
                    "Rolling p95 query latency in nanoseconds by bounded query kind",
                    kind,
                    percentile(&samples, 0.95),
                );
                write_gauge_labeled(
                    &mut out,
                    "rustyred_query_latency_p99_nanos",
                    "Rolling p99 query latency in nanoseconds by bounded query kind",
                    kind,
                    percentile(&samples, 0.99),
                );
            }
        }
        // §P7-A pa7.1 + pa7.2: emit one Prometheus histogram per recorded kind.
        // Algorithm kinds use the `kind` suffix in the metric name to keep
        // the `algorithm_latency_seconds{kind=...}` convention even though we
        // emit one series per kind here.
        if let Ok(histograms) = self.histograms.lock() {
            for (kind, histogram) in histograms.iter() {
                let metric_name = histogram_metric_name(kind);
                histogram.render(
                    &mut out,
                    &metric_name,
                    &format!("Latency histogram (seconds) for {kind}"),
                );
            }
        }
        out
    }
}

fn histogram_metric_name(kind: &str) -> String {
    // Map kind labels to Prometheus metric names:
    // - "cypher" -> "rustyred_cypher_latency_seconds"
    // - "vector_search" -> "rustyred_vector_search_latency_seconds"
    // - "fulltext_search" -> "rustyred_fulltext_search_latency_seconds"
    // - "algorithm:ppr" -> "rustyred_algorithm_latency_seconds_ppr" (one metric
    //   per algorithm kind so dashboards can compare per-algorithm budgets)
    if let Some(rest) = kind.strip_prefix("algorithm:") {
        format!("rustyred_algorithm_latency_seconds_{rest}")
    } else {
        format!("rustyred_{kind}_latency_seconds")
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

fn write_gauge_labeled(out: &mut String, name: &str, help: &str, kind: &str, value: u64) {
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(" gauge\n");
    out.push_str(name);
    out.push_str("{kind=\"");
    out.push_str(&escape_label(kind));
    out.push_str("\"} ");
    out.push_str(&value.to_string());
    out.push('\n');
}

fn percentile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * q).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn escape_label(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.'))
        .collect()
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
        assert!(prom.contains("rustyred_total_requests 2"));
        assert!(prom.contains("rustyred_cache_hits 1"));
        assert!(prom.contains("rustyred_vector_search_calls 1"));
        assert!(prom.contains("rustyred_ppr_calls 1"));
        assert!(prom.contains("# TYPE rustyred_total_requests counter"));
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

    #[test]
    fn latency_histograms_render_percentiles_for_fast_and_slow_queries() {
        let obs = Observability::new(1_000, 16);
        for nanos in [10, 20, 30, 40, 50] {
            obs.record_query_timing("cypher", "q", nanos, 0, 0);
        }

        let prom = obs.render_prometheus();

        assert!(prom.contains("rustyred_query_latency_count{kind=\"cypher\"} 5"));
        assert!(prom.contains("rustyred_query_latency_p50_nanos{kind=\"cypher\"} 30"));
        assert!(prom.contains("rustyred_query_latency_p95_nanos{kind=\"cypher\"} 50"));
    }

    #[test]
    fn slow_query_jsonl_export_writes_one_line_per_slow_query() {
        let path = std::env::temp_dir().join(format!("rusty-red-slow-{}.jsonl", unix_ms()));
        let obs = Observability::new_with_log(100, 16, Some(path.display().to_string()));

        obs.record_query_timing("cypher", "slow", 200, 1, 2);

        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 1);
        assert!(raw.contains("\"kind\":\"cypher\""));
        std::fs::remove_file(path).ok();
    }

    // ---- §P7-A pa7.1 histogram unit tests --------------------------------

    #[test]
    fn histogram_observe_increments_correct_bucket() {
        let h = Histogram::new(LATENCY_BUCKETS_SECONDS);
        h.observe_nanos(1_500_000); // 1.5 ms -> bucket 0.005 (and higher)
        h.observe_nanos(50_000_000); // 50 ms -> bucket 0.05 (and higher)
        h.observe_nanos(2_500_000_000); // 2.5 s -> bucket 2.5 (and higher)
        assert_eq!(h.count(), 3);
        assert_eq!(h.sum_nanos(), 1_500_000 + 50_000_000 + 2_500_000_000);
    }

    #[test]
    fn histogram_render_emits_bucket_sum_and_count_lines() {
        let h = Histogram::new(LATENCY_BUCKETS_SECONDS);
        h.observe_nanos(3_000_000); // 3 ms
        let mut out = String::new();
        h.render(
            &mut out,
            "rustyred_test_latency_seconds",
            "test latency histogram",
        );
        assert!(out.contains("rustyred_test_latency_seconds_bucket{le=\"0.005\"}"));
        assert!(out.contains("rustyred_test_latency_seconds_bucket{le=\"+Inf\"} 1"));
        assert!(out.contains("rustyred_test_latency_seconds_sum "));
        assert!(out.contains("rustyred_test_latency_seconds_count 1"));
        assert!(out.contains("# TYPE rustyred_test_latency_seconds histogram"));
    }

    #[test]
    fn render_prometheus_includes_cypher_histogram_after_timing_recorded() {
        let obs = Observability::default();
        obs.record_query_timing(KIND_CYPHER, "MATCH (n) RETURN n", 12_000_000, 0, 0);
        let prom = obs.render_prometheus();
        assert!(
            prom.contains("rustyred_cypher_latency_seconds_count 1"),
            "expected cypher histogram count in /metrics output:\n{prom}",
        );
        assert!(prom.contains("# TYPE rustyred_cypher_latency_seconds histogram"));
    }

    #[test]
    fn render_prometheus_includes_algorithm_histogram_per_kind() {
        let obs = Observability::default();
        obs.record_query_timing(KIND_ALGO_PAGERANK, "pagerank", 4_000_000, 0, 0);
        obs.record_query_timing(KIND_ALGO_COMPONENTS, "components", 7_000_000, 0, 0);
        let prom = obs.render_prometheus();
        assert!(prom.contains("rustyred_algorithm_latency_seconds_pagerank_count 1"));
        assert!(prom.contains("rustyred_algorithm_latency_seconds_components_count 1"));
    }
}
