use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::SubstrateSearch;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum CrawlDial {
    #[default]
    Conservative,
    Broad,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TriggerGateConfig {
    pub dial: CrawlDial,
    pub min_hits: usize,
    pub min_distinct_domains: usize,
    pub min_mean_score: f64,
    pub max_score_variance: f64,
    pub max_crawl_seeds: usize,
}

impl TriggerGateConfig {
    pub fn conservative() -> Self {
        Self {
            dial: CrawlDial::Conservative,
            min_hits: 1,
            min_distinct_domains: 1,
            min_mean_score: 0.01,
            max_score_variance: 1.0,
            max_crawl_seeds: 3,
        }
    }

    pub fn broad() -> Self {
        Self {
            dial: CrawlDial::Broad,
            min_hits: 3,
            min_distinct_domains: 2,
            min_mean_score: 0.02,
            max_score_variance: 0.5,
            max_crawl_seeds: 8,
        }
    }
}

impl Default for TriggerGateConfig {
    fn default() -> Self {
        Self::conservative()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TriggerGateDecision {
    pub should_crawl: bool,
    pub reason: String,
    pub hit_count: usize,
    pub distinct_domains: usize,
    pub mean_score: f64,
    pub score_variance: f64,
    pub max_crawl_seeds: usize,
}

pub fn evaluate_trigger_gate(
    search: &SubstrateSearch,
    config: &TriggerGateConfig,
) -> TriggerGateDecision {
    let stats = SearchStats::from_search(search);
    let (should_crawl, reason) = if search.query.trim().is_empty() {
        (false, "browse mode does not trigger crawl")
    } else if stats.hit_count == 0 {
        (true, "empty results")
    } else if stats.hit_count < config.min_hits {
        (true, "too few results")
    } else if stats.distinct_domains < config.min_distinct_domains {
        (true, "too little domain coverage")
    } else if stats.mean_score < config.min_mean_score {
        (true, "low mean graph score")
    } else if stats.score_variance > config.max_score_variance {
        (true, "score variance is too high")
    } else {
        (false, "local substrate evidence is sufficient")
    };

    TriggerGateDecision {
        should_crawl,
        reason: reason.to_string(),
        hit_count: stats.hit_count,
        distinct_domains: stats.distinct_domains,
        mean_score: stats.mean_score,
        score_variance: stats.score_variance,
        max_crawl_seeds: config.max_crawl_seeds,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SearchStats {
    hit_count: usize,
    distinct_domains: usize,
    mean_score: f64,
    score_variance: f64,
}

impl SearchStats {
    fn from_search(search: &SubstrateSearch) -> Self {
        if search.hits.is_empty() {
            return Self::default();
        }

        let mut domains = BTreeSet::new();
        let mut score_sum = 0.0;
        for hit in &search.hits {
            if let Ok(url) = Url::parse(&hit.url) {
                if let Some(host) = url.host_str() {
                    domains.insert(host.to_ascii_lowercase());
                }
            }
            score_sum += hit.match_score;
        }
        let hit_count = search.hits.len();
        let mean_score = score_sum / hit_count as f64;
        let variance = search
            .hits
            .iter()
            .map(|hit| {
                let delta = hit.match_score - mean_score;
                delta * delta
            })
            .sum::<f64>()
            / hit_count as f64;

        Self {
            hit_count,
            distinct_domains: domains.len(),
            mean_score,
            score_variance: variance,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SearchHit, SearchLink};

    fn hit(url: &str, score: f64) -> SearchHit {
        SearchHit {
            node_id: url.to_string(),
            url: url.to_string(),
            title: url.to_string(),
            snippet: String::new(),
            ring: 0,
            ring_label: "match".to_string(),
            match_score: score,
        }
    }

    fn search(query: &str, hits: Vec<SearchHit>) -> SubstrateSearch {
        SubstrateSearch {
            query: query.to_string(),
            hits,
            links: Vec::<SearchLink>::new(),
            matched_count: 0,
            kept_count: 0,
        }
    }

    #[test]
    fn empty_results_trigger_crawl() {
        let decision =
            evaluate_trigger_gate(&search("rust", vec![]), &TriggerGateConfig::default());
        assert!(decision.should_crawl);
        assert_eq!(decision.reason, "empty results");
    }

    #[test]
    fn conservative_gate_accepts_one_strong_result() {
        let decision = evaluate_trigger_gate(
            &search("rust", vec![hit("https://example.com/rust", 0.3)]),
            &TriggerGateConfig::conservative(),
        );
        assert!(!decision.should_crawl);
    }

    #[test]
    fn broad_gate_requires_more_coverage() {
        let decision = evaluate_trigger_gate(
            &search("rust", vec![hit("https://example.com/rust", 0.3)]),
            &TriggerGateConfig::broad(),
        );
        assert!(decision.should_crawl);
        assert_eq!(decision.reason, "too few results");
        assert_eq!(decision.max_crawl_seeds, 8);
    }
}
