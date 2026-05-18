use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use rustyred_core::{stable_hash, GraphStoreError};

const SUPPORTED_CACHE_KINDS: &[&str] = &[
    "query_result",
    "query_plan",
    "bounded_subgraph",
    "neighbor_expansion",
    "context_pack",
    "retrieval_plan",
    "semantic_answer_candidate",
    "modal_parse_result",
    "vector_search_result",
    "epistemic_traversal",
];

#[derive(Clone, Debug, Deserialize)]
pub struct GraphCacheLookupBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
    pub kind: String,
    pub key: Value,
    #[serde(default)]
    pub index_manifest_hash: Option<String>,
    #[serde(default)]
    pub auth_scope_hash: Option<String>,
    #[serde(default)]
    pub retrieval_policy_hash: Option<String>,
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub source_hashes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GraphCachePutBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
    pub kind: String,
    pub key: Value,
    pub value: Value,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub index_manifest_hash: Option<String>,
    #[serde(default)]
    pub auth_scope_hash: Option<String>,
    #[serde(default)]
    pub retrieval_policy_hash: Option<String>,
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub source_hashes: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GraphCacheInvalidateBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub stale_only: bool,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub key: Option<Value>,
    #[serde(default)]
    pub index_manifest_hash: Option<String>,
    #[serde(default)]
    pub auth_scope_hash: Option<String>,
    #[serde(default)]
    pub retrieval_policy_hash: Option<String>,
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub source_hashes: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GraphCacheStatsBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphCachePutResult {
    pub stored: bool,
    pub kind: String,
    pub fingerprint: String,
    pub cache_key: String,
    pub graph_version: u64,
    pub stored_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphCacheLookupResult {
    pub hit: bool,
    pub accepted: bool,
    pub stale: bool,
    pub kind: String,
    pub fingerprint: String,
    pub cache_key: String,
    pub reason: String,
    pub graph_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_graph_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub guards: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphCacheInvalidateResult {
    pub removed: usize,
    pub remaining: usize,
    pub stale_only: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphCacheStatsReport {
    pub graph_version: u64,
    pub entries_total: usize,
    pub stale_entries: usize,
    pub puts: u64,
    pub hits: u64,
    pub misses: u64,
    pub stale_hits: u64,
    pub invalidations: u64,
    pub entries_by_kind: BTreeMap<String, usize>,
}

#[derive(Debug, Default)]
struct GraphCacheState {
    entries: BTreeMap<String, GraphCacheEntry>,
    puts: u64,
    hits: u64,
    misses: u64,
    stale_hits: u64,
    invalidations: u64,
}

#[derive(Clone, Debug)]
struct GraphCacheEntry {
    kind: String,
    fingerprint: String,
    cache_key: String,
    graph_version: u64,
    stored_at_ms: u64,
    value: Value,
    metadata: Value,
}

#[derive(Clone, Debug, Serialize)]
struct GraphCacheFingerprintInput<'a> {
    kind: &'a str,
    key: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_manifest_hash: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_scope_hash: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retrieval_policy_hash: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_version: Option<&'a str>,
    source_hashes: &'a [String],
}

#[derive(Clone, Debug)]
struct NormalizedGraphCacheRequest {
    kind: String,
    fingerprint: String,
}

#[derive(Debug, Default)]
pub struct GraphCacheTenant {
    inner: Mutex<GraphCacheState>,
}

impl GraphCacheTenant {
    pub fn put(
        &self,
        body: GraphCachePutBody,
        current_graph_version: u64,
    ) -> Result<GraphCachePutResult, GraphStoreError> {
        let normalized = NormalizedGraphCacheRequest::from_lookup_body(GraphCacheLookupBody {
            tenant_id: body.tenant_id,
            kind: body.kind,
            key: body.key,
            index_manifest_hash: body.index_manifest_hash,
            auth_scope_hash: body.auth_scope_hash,
            retrieval_policy_hash: body.retrieval_policy_hash,
            model_version: body.model_version,
            source_hashes: body.source_hashes,
        })?;
        let stored_at_ms = now_epoch_ms();
        let cache_key = compose_cache_key(&normalized.fingerprint, current_graph_version);
        let entry = GraphCacheEntry {
            kind: normalized.kind.clone(),
            fingerprint: normalized.fingerprint.clone(),
            cache_key: cache_key.clone(),
            graph_version: current_graph_version,
            stored_at_ms,
            value: body.value,
            metadata: body.metadata,
        };
        let mut state = self.lock_state()?;
        state.entries.insert(normalized.fingerprint.clone(), entry);
        state.puts += 1;
        Ok(GraphCachePutResult {
            stored: true,
            kind: normalized.kind,
            fingerprint: normalized.fingerprint,
            cache_key,
            graph_version: current_graph_version,
            stored_at_ms,
        })
    }

    pub fn check(
        &self,
        body: GraphCacheLookupBody,
        current_graph_version: u64,
    ) -> Result<GraphCacheLookupResult, GraphStoreError> {
        self.lookup(body, current_graph_version, false)
    }

    pub fn get(
        &self,
        body: GraphCacheLookupBody,
        current_graph_version: u64,
    ) -> Result<GraphCacheLookupResult, GraphStoreError> {
        self.lookup(body, current_graph_version, true)
    }

    pub fn explain(
        &self,
        body: GraphCacheLookupBody,
        current_graph_version: u64,
    ) -> Result<GraphCacheLookupResult, GraphStoreError> {
        self.lookup(body, current_graph_version, false)
    }

    pub fn invalidate(
        &self,
        body: GraphCacheInvalidateBody,
        current_graph_version: u64,
    ) -> Result<GraphCacheInvalidateResult, GraphStoreError> {
        let filter = GraphCacheInvalidationFilter::from_body(body)?;
        let mut state = self.lock_state()?;
        let matching = state
            .entries
            .iter()
            .filter_map(|(fingerprint, entry)| {
                let stale = entry.graph_version != current_graph_version;
                if filter.matches(entry, fingerprint, stale) {
                    Some(fingerprint.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let removed = matching.len();
        for fingerprint in matching {
            state.entries.remove(&fingerprint);
        }
        if removed > 0 {
            state.invalidations += removed as u64;
        }
        Ok(GraphCacheInvalidateResult {
            removed,
            remaining: state.entries.len(),
            stale_only: filter.stale_only,
        })
    }

    pub fn stats(
        &self,
        current_graph_version: u64,
    ) -> Result<GraphCacheStatsReport, GraphStoreError> {
        let state = self.lock_state()?;
        let mut entries_by_kind = BTreeMap::new();
        let mut stale_entries = 0;
        for entry in state.entries.values() {
            *entries_by_kind.entry(entry.kind.clone()).or_insert(0) += 1;
            if entry.graph_version != current_graph_version {
                stale_entries += 1;
            }
        }
        Ok(GraphCacheStatsReport {
            graph_version: current_graph_version,
            entries_total: state.entries.len(),
            stale_entries,
            puts: state.puts,
            hits: state.hits,
            misses: state.misses,
            stale_hits: state.stale_hits,
            invalidations: state.invalidations,
            entries_by_kind,
        })
    }

    fn lookup(
        &self,
        body: GraphCacheLookupBody,
        current_graph_version: u64,
        include_value: bool,
    ) -> Result<GraphCacheLookupResult, GraphStoreError> {
        let normalized = NormalizedGraphCacheRequest::from_lookup_body(body)?;
        let current_cache_key = compose_cache_key(&normalized.fingerprint, current_graph_version);
        let mut state = self.lock_state()?;
        let Some(entry) = state.entries.get(&normalized.fingerprint).cloned() else {
            state.misses += 1;
            return Ok(GraphCacheLookupResult {
                hit: false,
                accepted: false,
                stale: false,
                kind: normalized.kind,
                fingerprint: normalized.fingerprint,
                cache_key: current_cache_key,
                reason: "miss".to_string(),
                graph_version: current_graph_version,
                entry_graph_version: None,
                entry_cache_key: None,
                stored_at_ms: None,
                value: None,
                metadata: None,
                guards: guard_map("graph_version", "missing"),
            });
        };
        if entry.graph_version != current_graph_version {
            state.stale_hits += 1;
            return Ok(GraphCacheLookupResult {
                hit: true,
                accepted: false,
                stale: true,
                kind: entry.kind,
                fingerprint: entry.fingerprint,
                cache_key: current_cache_key,
                reason: "graph_version_mismatch".to_string(),
                graph_version: current_graph_version,
                entry_graph_version: Some(entry.graph_version),
                entry_cache_key: Some(entry.cache_key),
                stored_at_ms: Some(entry.stored_at_ms),
                value: None,
                metadata: None,
                guards: guard_map("graph_version", "stale"),
            });
        }
        state.hits += 1;
        Ok(GraphCacheLookupResult {
            hit: true,
            accepted: true,
            stale: false,
            kind: entry.kind,
            fingerprint: entry.fingerprint,
            cache_key: entry.cache_key,
            reason: "exact_graph_version_match".to_string(),
            graph_version: current_graph_version,
            entry_graph_version: Some(entry.graph_version),
            entry_cache_key: Some(compose_cache_key(
                &normalized.fingerprint,
                entry.graph_version,
            )),
            stored_at_ms: Some(entry.stored_at_ms),
            value: if include_value {
                Some(entry.value)
            } else {
                None
            },
            metadata: if include_value {
                Some(entry.metadata)
            } else {
                None
            },
            guards: guard_map("graph_version", "match"),
        })
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, GraphCacheState>, GraphStoreError> {
        self.inner.lock().map_err(|_| {
            GraphStoreError::new(
                "graph_cache_lock_poisoned",
                "graph cache tenant lock poisoned",
            )
        })
    }
}

impl NormalizedGraphCacheRequest {
    fn from_lookup_body(body: GraphCacheLookupBody) -> Result<Self, GraphStoreError> {
        let kind = normalize_kind(&body.kind)?;
        if body.key.is_null() {
            return Err(GraphStoreError::new(
                "invalid_graph_cache_request",
                "cache key is required",
            ));
        }
        let source_hashes = normalize_string_list(body.source_hashes);
        let fingerprint = stable_hash(json!(GraphCacheFingerprintInput {
            kind: &kind,
            key: &body.key,
            index_manifest_hash: normalize_optional_str(body.index_manifest_hash.as_deref()),
            auth_scope_hash: normalize_optional_str(body.auth_scope_hash.as_deref()),
            retrieval_policy_hash: normalize_optional_str(body.retrieval_policy_hash.as_deref()),
            model_version: normalize_optional_str(body.model_version.as_deref()),
            source_hashes: &source_hashes,
        }));
        Ok(Self { kind, fingerprint })
    }
}

#[derive(Clone, Debug)]
struct GraphCacheInvalidationFilter {
    all: bool,
    stale_only: bool,
    kind: Option<String>,
    fingerprint: Option<String>,
}

impl GraphCacheInvalidationFilter {
    fn from_body(body: GraphCacheInvalidateBody) -> Result<Self, GraphStoreError> {
        let kind = match body.kind {
            Some(kind) => Some(normalize_kind(&kind)?),
            None => None,
        };
        let fingerprint = match body.key {
            Some(key) => {
                let lookup = GraphCacheLookupBody {
                    tenant_id: body.tenant_id,
                    kind: kind.clone().ok_or_else(|| {
                        GraphStoreError::new(
                            "invalid_graph_cache_request",
                            "cache invalidation with key requires kind",
                        )
                    })?,
                    key,
                    index_manifest_hash: body.index_manifest_hash,
                    auth_scope_hash: body.auth_scope_hash,
                    retrieval_policy_hash: body.retrieval_policy_hash,
                    model_version: body.model_version,
                    source_hashes: body.source_hashes,
                };
                Some(NormalizedGraphCacheRequest::from_lookup_body(lookup)?.fingerprint)
            }
            None => None,
        };
        if !body.all && !body.stale_only && kind.is_none() && fingerprint.is_none() {
            return Err(GraphStoreError::new(
                "invalid_graph_cache_request",
                "cache invalidation requires all=true, stale_only=true, kind, or key",
            ));
        }
        Ok(Self {
            all: body.all,
            stale_only: body.stale_only,
            kind,
            fingerprint,
        })
    }

    fn matches(&self, entry: &GraphCacheEntry, fingerprint: &str, stale: bool) -> bool {
        if !self.all {
            if self.stale_only && !stale {
                return false;
            }
            if let Some(kind) = self.kind.as_deref() {
                if entry.kind != kind {
                    return false;
                }
            }
            if let Some(expected) = self.fingerprint.as_deref() {
                if fingerprint != expected {
                    return false;
                }
            }
            return self.stale_only || self.kind.is_some() || self.fingerprint.is_some();
        }
        !self.stale_only || stale
    }
}

fn normalize_kind(kind: &str) -> Result<String, GraphStoreError> {
    let normalized = match kind.trim().to_ascii_lowercase().as_str() {
        "query" | "query_result" => "query_result",
        "plan" | "query_plan" => "query_plan",
        "subgraph" | "bounded_subgraph" => "bounded_subgraph",
        "neighbors" | "neighbor_expansion" => "neighbor_expansion",
        "pack" | "context_pack" => "context_pack",
        "retrieval" | "retrieval_plan" => "retrieval_plan",
        "semantic_answer" | "semantic_answer_candidate" => "semantic_answer_candidate",
        "modal_parse" | "modal_parse_result" => "modal_parse_result",
        // §P6-A pa6.2: explicit normalization for the two cache kinds the SPEC
        // requires participate in graph_version invalidation. Without these
        // arms, both kinds dropped through to the empty-string fallthrough and
        // were rejected as `unsupported_graph_cache_kind`.
        "vector_search" | "vector_search_result" => "vector_search_result",
        "epistemic" | "epistemic_traversal" => "epistemic_traversal",
        _ => "",
    };
    if normalized.is_empty() || !SUPPORTED_CACHE_KINDS.contains(&normalized) {
        return Err(GraphStoreError::new(
            "unsupported_graph_cache_kind",
            format!("unsupported graph cache kind: {kind}"),
        ));
    }
    Ok(normalized.to_string())
}

fn normalize_optional_str(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let mut normalized = BTreeSet::new();
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            normalized.insert(trimmed.to_string());
        }
    }
    normalized.into_iter().collect()
}

fn compose_cache_key(fingerprint: &str, graph_version: u64) -> String {
    stable_hash(json!({
        "fingerprint": fingerprint,
        "graph_version": graph_version,
    }))
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn guard_map(key: &str, value: &str) -> BTreeMap<String, String> {
    let mut guards = BTreeMap::new();
    guards.insert(key.to_string(), value.to_string());
    guards
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        GraphCacheInvalidateBody, GraphCacheLookupBody, GraphCachePutBody, GraphCacheTenant,
    };

    #[test]
    fn graph_cache_hits_when_graph_version_matches() {
        let cache = GraphCacheTenant::default();
        cache
            .put(
                GraphCachePutBody {
                    tenant_id: None,
                    kind: "query".to_string(),
                    key: json!({ "label": "File", "path": "src/lib.rs" }),
                    value: json!({ "nodes": ["node:a"] }),
                    metadata: json!({ "operation": "node_match" }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                3,
            )
            .unwrap();

        let result = cache
            .get(
                GraphCacheLookupBody {
                    tenant_id: None,
                    kind: "query_result".to_string(),
                    key: json!({ "label": "File", "path": "src/lib.rs" }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                3,
            )
            .unwrap();

        assert!(result.hit);
        assert!(result.accepted);
        assert!(!result.stale);
        assert_eq!(result.reason, "exact_graph_version_match");
        assert_eq!(result.value, Some(json!({ "nodes": ["node:a"] })));
    }

    #[test]
    fn graph_cache_reports_stale_hit_after_graph_version_changes() {
        let cache = GraphCacheTenant::default();
        cache
            .put(
                GraphCachePutBody {
                    tenant_id: None,
                    kind: "context_pack".to_string(),
                    key: json!({ "object_id": "file:lib" }),
                    value: json!({ "pack": ["file:lib", "edge:ab"] }),
                    metadata: json!({}),
                    index_manifest_hash: None,
                    auth_scope_hash: Some("public".to_string()),
                    retrieval_policy_hash: Some("default".to_string()),
                    model_version: None,
                    source_hashes: vec!["src/lib.rs".to_string()],
                },
                5,
            )
            .unwrap();

        let result = cache
            .check(
                GraphCacheLookupBody {
                    tenant_id: None,
                    kind: "pack".to_string(),
                    key: json!({ "object_id": "file:lib" }),
                    index_manifest_hash: None,
                    auth_scope_hash: Some("public".to_string()),
                    retrieval_policy_hash: Some("default".to_string()),
                    model_version: None,
                    source_hashes: vec!["src/lib.rs".to_string()],
                },
                6,
            )
            .unwrap();

        assert!(result.hit);
        assert!(!result.accepted);
        assert!(result.stale);
        assert_eq!(result.reason, "graph_version_mismatch");
        assert_eq!(result.entry_graph_version, Some(5));
        assert!(result.value.is_none());
    }

    #[test]
    fn graph_cache_invalidate_removes_matching_entries() {
        let cache = GraphCacheTenant::default();
        cache
            .put(
                GraphCachePutBody {
                    tenant_id: None,
                    kind: "query_plan".to_string(),
                    key: json!({ "query": "MATCH (n:File) RETURN n LIMIT 10" }),
                    value: json!({ "plan": ["node_index_seek"] }),
                    metadata: json!({}),
                    index_manifest_hash: Some("labels:v1".to_string()),
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                9,
            )
            .unwrap();

        let result = cache
            .invalidate(
                GraphCacheInvalidateBody {
                    tenant_id: None,
                    all: false,
                    stale_only: false,
                    kind: Some("plan".to_string()),
                    key: Some(json!({ "query": "MATCH (n:File) RETURN n LIMIT 10" })),
                    index_manifest_hash: Some("labels:v1".to_string()),
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                9,
            )
            .unwrap();

        assert_eq!(result.removed, 1);
        assert_eq!(result.remaining, 0);
        assert_eq!(cache.stats(9).unwrap().invalidations, 1);
    }

    // §P6-A pa6.2: confirm `vector_search_result` and `epistemic_traversal`
    // cache kinds participate in the same `graph_version` invalidation flow
    // that powers every other supported kind.
    #[test]
    fn vector_search_result_cache_kind_invalidates_on_graph_version() {
        let cache = GraphCacheTenant::default();
        cache
            .put(
                GraphCachePutBody {
                    tenant_id: None,
                    kind: "vector_search_result".to_string(),
                    key: json!({ "label": "Doc", "query": "lorem" }),
                    value: json!({ "results": ["node:a"] }),
                    metadata: json!({ "operation": "vector_search" }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                5,
            )
            .unwrap();

        // Same graph_version: fresh hit.
        let fresh = cache
            .get(
                GraphCacheLookupBody {
                    tenant_id: None,
                    kind: "vector_search_result".to_string(),
                    key: json!({ "label": "Doc", "query": "lorem" }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                5,
            )
            .unwrap();
        assert!(fresh.accepted && !fresh.stale);

        // Bump graph_version: cache must mark the entry stale.
        let stale = cache
            .get(
                GraphCacheLookupBody {
                    tenant_id: None,
                    kind: "vector_search_result".to_string(),
                    key: json!({ "label": "Doc", "query": "lorem" }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                6,
            )
            .unwrap();
        assert!(
            stale.stale,
            "vector_search_result entry must become stale at the next graph_version",
        );
    }

    #[test]
    fn epistemic_traversal_cache_kind_invalidates_on_graph_version() {
        let cache = GraphCacheTenant::default();
        cache
            .put(
                GraphCachePutBody {
                    tenant_id: None,
                    kind: "epistemic_traversal".to_string(),
                    key: json!({ "node_id": "node:root", "max_depth": 2 }),
                    value: json!({ "hits": ["node:leaf"] }),
                    metadata: json!({ "operation": "epistemic_neighbors" }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                12,
            )
            .unwrap();

        let fresh = cache
            .get(
                GraphCacheLookupBody {
                    tenant_id: None,
                    kind: "epistemic_traversal".to_string(),
                    key: json!({ "node_id": "node:root", "max_depth": 2 }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                12,
            )
            .unwrap();
        assert!(fresh.accepted && !fresh.stale);

        let stale = cache
            .get(
                GraphCacheLookupBody {
                    tenant_id: None,
                    kind: "epistemic_traversal".to_string(),
                    key: json!({ "node_id": "node:root", "max_depth": 2 }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: None,
                    source_hashes: Vec::new(),
                },
                13,
            )
            .unwrap();
        assert!(
            stale.stale,
            "epistemic_traversal entry must become stale at the next graph_version",
        );
    }
}
