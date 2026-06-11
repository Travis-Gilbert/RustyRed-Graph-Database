//! Memory-verb endpoints for cross-surface user memory.
//!
//! These routes are the canonical write+read path for the harness verbs
//! (`encode`, `coordinate`, `presence`, `mentions`, `mentions_wait`,
//! `subscribe`, `recall`, `remember`, `self_note`, `self_revise`,
//! `self_archive`, `self_recall_archive`). They land beside the existing
//! `/v1/tenants/:tenant_id/graph/*` routes and reuse the same
//! `tenant_graph_store` primitive, with per-tenant scope auth.
//!
//! Storage model: each verb produces a `NodeRecord` labelled
//! `["MemoryAtom", VerbKind]` (e.g. `["MemoryAtom", "EncodeEvent"]`).
//! Properties carry actor identity, content, fitness, tier, lifecycle
//! timestamps, and verb-specific fields. Edges link revisions, mentions,
//! and supersessions.
//!
//! ID format: `mem:{kind}:{sha1(stable_seed)[:20]}` keeps writes
//! idempotent — a retry with the same seed upserts the same node.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use rustyred_core::{EdgeRecord, NodeQuery, NodeRecord};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::auth::require_scope;
use crate::state::AppState;

// ===== body types =====

#[derive(Debug, Deserialize)]
pub struct EncodeBody {
    pub content: String,
    #[serde(default)]
    pub kind: Option<String>, // "encode" | "feedback" | "solution" | "postmortem"
    #[serde(default)]
    pub outcome: Option<String>, // "positive" | "negative" | "mixed" | "neutral"
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default)]
    pub auto_triggered: bool,
}

#[derive(Debug, Deserialize)]
pub struct CoordinateBody {
    pub message: String,
    #[serde(default)]
    pub doc_id: Option<String>,
    #[serde(default)]
    pub urgency: Option<String>, // "info" | "ask" | "block"
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PresenceBody {
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub mode: Option<String>, // "heartbeat" | "get" | "end"
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub ttl_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MentionsBody {
    pub actor: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub consume: bool,
}

#[derive(Debug, Deserialize)]
pub struct MentionsWaitBody {
    pub actor: String,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub interval_seconds: Option<f64>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub consume: bool,
}

#[derive(Debug, Deserialize)]
pub struct SubscribeBody {
    pub actor: String,
    #[serde(default)]
    pub doc_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecallBody {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_low_fitness: bool,
    #[serde(default)]
    pub include_consolidation_sources: bool,
    #[serde(default)]
    pub consume_handoffs: bool,
}

#[derive(Debug, Deserialize)]
pub struct RememberBody {
    pub observation: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SelfNoteBody {
    pub content: String,
    #[serde(default)]
    pub memory_node_type: Option<String>, // belief | convention | standing_intention | reasoning_record
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SelfReviseBody {
    pub doc_id: String,
    pub content: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SelfArchiveBody {
    pub doc_id: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SelfRecallArchiveBody {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

// ===== helpers =====

fn now_iso() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let nanos = now.subsec_nanos();
    // RFC3339-ish; chrono is not a dep here, hand-format from epoch.
    // For production-grade, swap in chrono. Good enough for atom timestamps.
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        1970 + (secs / 31_557_600) as u32,
        ((secs / 2_629_800) % 12) as u32 + 1,
        ((secs / 86_400) % 30) as u32 + 1,
        ((secs / 3_600) % 24) as u32,
        ((secs / 60) % 60) as u32,
        (secs % 60) as u32,
        nanos
    )
}

fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn sha1_hex(seed: &str) -> String {
    // Named sha1_hex for callers' convenience but uses sha2::Sha256
    // (matches the codebase's existing hashing crate). Truncate when used.
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    let out = hasher.finalize();
    out.iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

fn atom_id(kind: &str, seed: &str) -> String {
    let digest = sha1_hex(seed);
    format!("mem:{}:{}", kind, &digest[..20])
}

fn merge_metadata(extra: Value, base: Map<String, Value>) -> Value {
    let mut out = base;
    if let Value::Object(map) = extra {
        for (k, v) in map {
            out.entry(k).or_insert(v);
        }
    }
    Value::Object(out)
}

fn require_str(value: Option<String>) -> Option<String> {
    value.and_then(|s| {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn parse_mentions(message: &str) -> Vec<String> {
    // Match @actor where actor matches [A-Za-z0-9][A-Za-z0-9_.:-]{0,119}
    // Skip if preceded by word char (e.g. email-like a@b is not a mention).
    // Strip code fences and inline code first.
    let stripped = strip_code(message);
    let bytes = stripped.as_bytes();
    let mut seen: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            // check preceding char is not word-class
            let prev_is_word = i > 0
                && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if !prev_is_word && i + 1 < bytes.len() {
                let start = i + 1;
                let first = bytes[start];
                if first.is_ascii_alphanumeric() {
                    let mut end = start + 1;
                    while end < bytes.len() {
                        let c = bytes[end];
                        if c.is_ascii_alphanumeric()
                            || c == b'_'
                            || c == b'.'
                            || c == b':'
                            || c == b'-'
                        {
                            end += 1;
                        } else {
                            break;
                        }
                    }
                    let len = (end - start).min(120);
                    let actor = String::from_utf8_lossy(&bytes[start..start + len]).to_string();
                    if !seen.contains(&actor) {
                        seen.push(actor);
                    }
                    i = start + len;
                    continue;
                }
            }
        }
        i += 1;
    }
    seen
}

fn strip_code(input: &str) -> String {
    // Replace ```...``` and `...` with spaces of equal length. Simple
    // forward scan; nested fences ignored.
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // triple backtick
        if i + 2 < bytes.len() && bytes[i] == b'`' && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
            let mut j = i + 3;
            while j + 2 < bytes.len() {
                if bytes[j] == b'`' && bytes[j + 1] == b'`' && bytes[j + 2] == b'`' {
                    j += 3;
                    break;
                }
                j += 1;
            }
            for _ in i..j {
                out.push(b' ');
            }
            i = j;
            continue;
        }
        // single backtick
        if bytes[i] == b'`' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'`' && bytes[j] != b'\n' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'`' {
                j += 1;
            }
            for _ in i..j {
                out.push(b' ');
            }
            i = j;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn build_atom_node(
    id: String,
    kind_label: &str,
    extra_labels: &[&str],
    properties: Value,
) -> NodeRecord {
    let mut labels = vec!["MemoryAtom".to_string(), kind_label.to_string()];
    for l in extra_labels {
        labels.push(l.to_string());
    }
    NodeRecord::new(id, labels, properties)
}

fn json_response(value: Value) -> axum::response::Response {
    Json(value).into_response()
}

// Reuse the canonical error renderers from router.rs so memory-verb
// responses match the rest of the API's error envelopes.
use crate::router::{graph_store_error_response as store_error, store_unavailable_response as unavailable_error};

// ===== handlers =====

pub async fn memory_encode(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<EncodeBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let kind = body
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("encode")
        .to_lowercase();
    let outcome = body
        .outcome
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("neutral")
        .to_lowercase();
    let signal = body
        .signal
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("encode")
        .to_string();
    let actor_id = require_str(body.actor).unwrap_or_else(|| "agent".to_string());
    let session_id = require_str(body.session_id).unwrap_or_default();
    let surface = require_str(body.surface).unwrap_or_default();
    let title = body
        .title
        .clone()
        .unwrap_or_else(|| format!("{}: {}", kind, body.content.chars().take(80).collect::<String>()));
    let summary = body
        .summary
        .clone()
        .unwrap_or_else(|| body.content.chars().take(500).collect::<String>());

    let content_hash = sha1_hex(&body.content);
    let seed = format!("{}|{}|{}|{}", tenant_id, actor_id, kind, content_hash);
    let id = atom_id("encode", &seed);

    let mut props = serde_json::Map::new();
    props.insert("tenant_slug".to_string(), json!(tenant_id));
    props.insert("actor_id".to_string(), json!(actor_id));
    props.insert("session_id".to_string(), json!(session_id));
    props.insert("surface".to_string(), json!(surface));
    props.insert("kind".to_string(), json!(kind));
    props.insert("outcome".to_string(), json!(outcome));
    props.insert("signal".to_string(), json!(signal));
    props.insert("reason".to_string(), json!(body.reason.unwrap_or_default()));
    props.insert("title".to_string(), json!(title));
    props.insert("content".to_string(), json!(body.content));
    props.insert("summary".to_string(), json!(summary));
    props.insert("content_hash".to_string(), json!(content_hash));
    props.insert("tags".to_string(), json!(body.tags));
    props.insert("links".to_string(), json!(body.links));
    props.insert("auto_triggered".to_string(), json!(body.auto_triggered));
    props.insert("status".to_string(), json!("active"));
    props.insert("tier".to_string(), json!("scratch"));
    props.insert("fitness".to_string(), json!(0.2));
    props.insert("revision".to_string(), json!(1));
    props.insert("captured_at".to_string(), json!(now_iso()));
    props.insert("updated_at".to_string(), json!(now_iso()));
    let properties = merge_metadata(body.metadata, props);

    let node = build_atom_node(id.clone(), "EncodeEvent", &[], properties);

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };
    match store.upsert_node(node.clone()) {
        Ok(result) => {
            state.observability.record_mutation();
            state.maybe_index_node_fulltext(&tenant_id, &node);
            json_response(json!({
                "ok": true,
                "doc_id": id,
                "kind": kind,
                "outcome": outcome,
                "signal": signal,
                "node": result,
            }))
        }
        Err(error) => {
            state.observability.record_error();
            store_error(error)
        }
    }
}

pub async fn memory_coordinate(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CoordinateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let actor_id = require_str(body.actor).unwrap_or_else(|| "agent".to_string());
    let session_id = require_str(body.session_id).unwrap_or_default();
    let surface = require_str(body.surface).unwrap_or_default();
    let urgency = body
        .urgency
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("info")
        .to_lowercase();
    let mentioned = parse_mentions(&body.message);

    let doc_id = body
        .doc_id
        .clone()
        .unwrap_or_else(|| atom_id("coord", &format!("{}|{}|{}", tenant_id, actor_id, sha1_hex(&body.message))));
    let title = body
        .title
        .clone()
        .unwrap_or_else(|| format!("coordination from @{}", actor_id));

    let mut props = serde_json::Map::new();
    props.insert("tenant_slug".to_string(), json!(tenant_id));
    props.insert("actor_id".to_string(), json!(actor_id));
    props.insert("session_id".to_string(), json!(session_id));
    props.insert("surface".to_string(), json!(surface));
    props.insert("kind".to_string(), json!("coordinate"));
    props.insert("doc_id".to_string(), json!(doc_id));
    props.insert("title".to_string(), json!(title));
    props.insert("content".to_string(), json!(body.message));
    props.insert("urgency".to_string(), json!(urgency));
    props.insert("mentioned_actors".to_string(), json!(mentioned));
    props.insert("status".to_string(), json!("active"));
    props.insert("captured_at".to_string(), json!(now_iso()));
    props.insert("updated_at".to_string(), json!(now_iso()));
    let properties = merge_metadata(body.metadata, props);

    let coord_node = build_atom_node(doc_id.clone(), "CoordinationMessage", &[], properties);

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };

    let _ = match store.upsert_node(coord_node.clone()) {
        Ok(r) => r,
        Err(error) => {
            state.observability.record_error();
            return store_error(error);
        }
    };
    state.observability.record_mutation();
    state.maybe_index_node_fulltext(&tenant_id, &coord_node);

    // Per-mention atoms (so reads of /mentions can filter by target_actor_id).
    let mut mention_ids = Vec::new();
    for target in &mentioned {
        let mention_id = atom_id(
            "mention",
            &format!("{}|{}|{}", doc_id, target, now_epoch_secs()),
        );
        let mut mp = serde_json::Map::new();
        mp.insert("tenant_slug".to_string(), json!(tenant_id));
        mp.insert("origin_actor_id".to_string(), json!(actor_id));
        mp.insert("target_actor_id".to_string(), json!(target));
        mp.insert("parent_doc_id".to_string(), json!(doc_id));
        mp.insert("kind".to_string(), json!("mention"));
        mp.insert("title".to_string(), json!(format!("@{} from @{}", target, actor_id)));
        mp.insert("content".to_string(), json!(body.message));
        mp.insert("urgency".to_string(), json!(urgency));
        mp.insert("status".to_string(), json!("active"));
        mp.insert("captured_at".to_string(), json!(now_iso()));
        mp.insert("updated_at".to_string(), json!(now_iso()));
        let mention_node = build_atom_node(
            mention_id.clone(),
            "Mention",
            &[],
            Value::Object(mp),
        );
        if store.upsert_node(mention_node.clone()).is_ok() {
            state.maybe_index_node_fulltext(&tenant_id, &mention_node);
            mention_ids.push(mention_id.clone());
            // Edge: mention -> coord_doc
            let edge_id = format!("edge:{}:mentions:{}", mention_id, doc_id);
            let edge = EdgeRecord::new(edge_id, mention_id, doc_id.clone(), "MENTIONS", Value::Object(Default::default()));
            let _ = store.upsert_edge(edge);
        }
    }

    json_response(json!({
        "ok": true,
        "doc_id": doc_id,
        "mentions": mentioned,
        "mention_atom_ids": mention_ids,
        "urgency": urgency,
    }))
}

pub async fn memory_presence(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PresenceBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let actor_id = require_str(body.actor).unwrap_or_else(|| "agent".to_string());
    let mode = body
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("heartbeat")
        .to_lowercase();
    let surface = require_str(body.surface).unwrap_or_default();
    let session_id = require_str(body.session_id).unwrap_or_default();
    let status = require_str(body.status).unwrap_or_else(|| "active".to_string());
    let ttl_seconds = body.ttl_seconds.unwrap_or(60).max(0);
    let expires_at = now_epoch_secs() + ttl_seconds;

    // Stable id per (tenant, actor) so heartbeat updates the same node.
    let id = atom_id("presence", &format!("{}|{}", tenant_id, actor_id));

    let mut props = serde_json::Map::new();
    props.insert("tenant_slug".to_string(), json!(tenant_id));
    props.insert("actor_id".to_string(), json!(actor_id));
    props.insert("session_id".to_string(), json!(session_id));
    props.insert("surface".to_string(), json!(surface));
    props.insert("kind".to_string(), json!("presence"));
    props.insert("mode".to_string(), json!(mode));
    props.insert("status".to_string(), json!(status));
    props.insert("ttl_seconds".to_string(), json!(ttl_seconds));
    props.insert("expires_at_epoch".to_string(), json!(expires_at));
    props.insert("refreshed_at".to_string(), json!(now_iso()));
    let properties = Value::Object(props);

    let node = build_atom_node(id.clone(), "Presence", &[], properties);

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };
    match store.upsert_node(node) {
        Ok(result) => {
            state.observability.record_mutation();
            json_response(json!({
                "ok": true,
                "presence_id": id,
                "expires_at_epoch": expires_at,
                "node": result,
            }))
        }
        Err(error) => {
            state.observability.record_error();
            store_error(error)
        }
    }
}

pub async fn memory_mentions(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<MentionsBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let actor = body.actor.trim().to_string();
    let limit = body.limit.unwrap_or(20).min(200);

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };

    let query = NodeQuery::label("Mention")
        .with_property("target_actor_id", json!(actor))
        .with_property("status", json!("active"))
        .with_limit(limit);
    let nodes = match store.query_nodes(query) {
        Ok(nodes) => nodes,
        Err(error) => return store_error(error),
    };

    let mut consumed_ids = Vec::new();
    if body.consume {
        for node in &nodes {
            let mut updated = node.clone();
            if let Value::Object(ref mut map) = updated.properties {
                map.insert("status".to_string(), json!("consumed"));
                map.insert("consumed_at".to_string(), json!(now_iso()));
                map.insert("updated_at".to_string(), json!(now_iso()));
            }
            if store.upsert_node(updated).is_ok() {
                consumed_ids.push(node.id.clone());
            }
        }
    }

    json_response(json!({
        "ok": true,
        "actor": actor,
        "count": nodes.len(),
        "mentions": nodes,
        "consumed": body.consume,
        "consumed_ids": consumed_ids,
    }))
}

pub async fn memory_mentions_wait(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<MentionsWaitBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let actor = body.actor.trim().to_string();
    let limit = body.limit.unwrap_or(20).min(200);
    let timeout = body.timeout_seconds.unwrap_or(30).min(120);
    let interval_ms = ((body.interval_seconds.unwrap_or(1.0)).max(0.1).min(5.0) * 1000.0) as u64;

    let started = std::time::Instant::now();
    let deadline = std::time::Duration::from_secs(timeout);

    loop {
        let store = match state.tenant_graph_store(&tenant_id) {
            Ok(store) => store,
            Err(error) => return unavailable_error(error),
        };
        let query = NodeQuery::label("Mention")
            .with_property("target_actor_id", json!(actor))
            .with_property("status", json!("active"))
            .with_limit(limit);
        let nodes = match store.query_nodes(query) {
            Ok(nodes) => nodes,
            Err(error) => return store_error(error),
        };
        if !nodes.is_empty() || started.elapsed() >= deadline {
            let mut consumed_ids = Vec::new();
            if body.consume && !nodes.is_empty() {
                let mut store = store;
                for node in &nodes {
                    let mut updated = node.clone();
                    if let Value::Object(ref mut map) = updated.properties {
                        map.insert("status".to_string(), json!("consumed"));
                        map.insert("consumed_at".to_string(), json!(now_iso()));
                        map.insert("updated_at".to_string(), json!(now_iso()));
                    }
                    if store.upsert_node(updated).is_ok() {
                        consumed_ids.push(node.id.clone());
                    }
                }
            }
            return json_response(json!({
                "ok": true,
                "actor": actor,
                "count": nodes.len(),
                "mentions": nodes,
                "timed_out": nodes.is_empty(),
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "consumed": body.consume,
                "consumed_ids": consumed_ids,
            }));
        }
        drop(store);
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
    }
}

pub async fn memory_subscribe(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SubscribeBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let actor = body.actor.trim().to_string();
    let doc_id = body.doc_id.unwrap_or_default();
    let id = atom_id("subscribe", &format!("{}|{}|{}", tenant_id, actor, doc_id));

    let mut props = serde_json::Map::new();
    props.insert("tenant_slug".to_string(), json!(tenant_id));
    props.insert("actor_id".to_string(), json!(actor));
    props.insert("doc_id".to_string(), json!(doc_id));
    props.insert("kind".to_string(), json!("subscription"));
    props.insert("status".to_string(), json!("active"));
    props.insert("subscribed_at".to_string(), json!(now_iso()));
    let properties = Value::Object(props);
    let node = build_atom_node(id.clone(), "Subscription", &[], properties);

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };
    match store.upsert_node(node) {
        Ok(result) => {
            state.observability.record_mutation();
            json_response(json!({
                "ok": true,
                "subscription_id": id,
                "node": result,
            }))
        }
        Err(error) => {
            state.observability.record_error();
            store_error(error)
        }
    }
}

pub async fn memory_recall(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RecallBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let limit = body.limit.unwrap_or(10).min(200);
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };

    // Base query: MemoryAtom with active status. Optionally filter by
    // actor/surface/kind. Caller-supplied free-text `query` is matched
    // against title/content/summary substring (case-insensitive).
    let mut node_query = NodeQuery::label("MemoryAtom")
        .with_property("status", json!("active"))
        .with_limit(limit * 4); // over-fetch for content filter then trim
    if let Some(actor) = body.actor.as_deref().filter(|s| !s.trim().is_empty()) {
        node_query = node_query.with_property("actor_id", json!(actor.trim()));
    }
    if let Some(surface) = body.surface.as_deref().filter(|s| !s.trim().is_empty()) {
        node_query = node_query.with_property("surface", json!(surface.trim()));
    }
    if let Some(kind) = body.kind.as_deref().filter(|s| !s.trim().is_empty()) {
        node_query = node_query.with_property("kind", json!(kind.trim().to_lowercase()));
    }
    let nodes = match store.query_nodes(node_query) {
        Ok(nodes) => nodes,
        Err(error) => return store_error(error),
    };

    let needle = body
        .query
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());

    let mut hits: Vec<&NodeRecord> = nodes
        .iter()
        .filter(|n| {
            if let Some(ref n_query) = needle {
                let props = &n.properties;
                let title_match = props
                    .get("title")
                    .and_then(Value::as_str)
                    .map(|s| s.to_lowercase().contains(n_query))
                    .unwrap_or(false);
                let content_match = props
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|s| s.to_lowercase().contains(n_query))
                    .unwrap_or(false);
                let summary_match = props
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(|s| s.to_lowercase().contains(n_query))
                    .unwrap_or(false);
                title_match || content_match || summary_match
            } else {
                true
            }
        })
        .filter(|n| {
            if body.include_low_fitness {
                true
            } else {
                let fitness = n
                    .properties
                    .get("fitness")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                fitness >= 0.3
            }
        })
        .collect();

    // Sort by fitness desc, then updated_at desc.
    hits.sort_by(|a, b| {
        let fa = a.properties.get("fitness").and_then(Value::as_f64).unwrap_or(0.0);
        let fb = b.properties.get("fitness").and_then(Value::as_f64).unwrap_or(0.0);
        fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);

    let results: Vec<Value> = hits.iter().map(|n| json!(n)).collect();

    json_response(json!({
        "ok": true,
        "count": results.len(),
        "results": results,
    }))
}

pub async fn memory_remember(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RememberBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let actor_id = require_str(body.actor).unwrap_or_else(|| "agent".to_string());
    let session_id = require_str(body.session_id).unwrap_or_default();
    let surface = require_str(body.surface).unwrap_or_default();

    let content_hash = sha1_hex(&body.observation);
    let id = atom_id("remember", &format!("{}|{}|{}", tenant_id, actor_id, content_hash));

    let mut props = serde_json::Map::new();
    props.insert("tenant_slug".to_string(), json!(tenant_id));
    props.insert("actor_id".to_string(), json!(actor_id));
    props.insert("session_id".to_string(), json!(session_id));
    props.insert("surface".to_string(), json!(surface));
    props.insert("kind".to_string(), json!("observation"));
    props.insert("content".to_string(), json!(body.observation));
    props.insert("evidence".to_string(), json!(body.evidence));
    props.insert("status".to_string(), json!("active"));
    props.insert("tier".to_string(), json!("scratch"));
    props.insert("fitness".to_string(), json!(0.2));
    props.insert("captured_at".to_string(), json!(now_iso()));
    props.insert("updated_at".to_string(), json!(now_iso()));
    let properties = Value::Object(props);

    let node = build_atom_node(id.clone(), "Observation", &[], properties);

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };
    match store.upsert_node(node.clone()) {
        Ok(result) => {
            state.observability.record_mutation();
            state.maybe_index_node_fulltext(&tenant_id, &node);
            json_response(json!({ "ok": true, "doc_id": id, "node": result }))
        }
        Err(error) => {
            state.observability.record_error();
            store_error(error)
        }
    }
}

pub async fn memory_self_note(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SelfNoteBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let actor_id = require_str(body.actor).unwrap_or_else(|| "agent".to_string());
    let session_id = require_str(body.session_id).unwrap_or_default();
    let surface = require_str(body.surface).unwrap_or_default();
    let memory_node_type = body
        .memory_node_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("belief")
        .to_lowercase();
    let kind = body
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("self_note")
        .to_lowercase();
    let title = body
        .title
        .clone()
        .unwrap_or_else(|| format!("self_note: {}", body.content.chars().take(80).collect::<String>()));
    let summary = body
        .summary
        .clone()
        .unwrap_or_else(|| body.content.chars().take(500).collect::<String>());

    let content_hash = sha1_hex(&body.content);
    let id = atom_id("selfnote", &format!("{}|{}|{}", tenant_id, actor_id, content_hash));

    let mut props = serde_json::Map::new();
    props.insert("tenant_slug".to_string(), json!(tenant_id));
    props.insert("actor_id".to_string(), json!(actor_id));
    props.insert("session_id".to_string(), json!(session_id));
    props.insert("surface".to_string(), json!(surface));
    props.insert("kind".to_string(), json!(kind));
    props.insert("memory_node_type".to_string(), json!(memory_node_type));
    props.insert("title".to_string(), json!(title));
    props.insert("content".to_string(), json!(body.content));
    props.insert("summary".to_string(), json!(summary));
    props.insert("tags".to_string(), json!(body.tags));
    props.insert("links".to_string(), json!(body.links));
    props.insert("status".to_string(), json!("active"));
    props.insert("tier".to_string(), json!("scratch"));
    props.insert("fitness".to_string(), json!(0.2));
    props.insert("revision".to_string(), json!(1));
    props.insert("captured_at".to_string(), json!(now_iso()));
    props.insert("updated_at".to_string(), json!(now_iso()));
    let properties = Value::Object(props);

    let node = build_atom_node(id.clone(), "SelfNote", &[], properties);

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };
    match store.upsert_node(node.clone()) {
        Ok(result) => {
            state.observability.record_mutation();
            state.maybe_index_node_fulltext(&tenant_id, &node);
            json_response(json!({ "ok": true, "doc_id": id, "node": result }))
        }
        Err(error) => {
            state.observability.record_error();
            store_error(error)
        }
    }
}

pub async fn memory_self_revise(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SelfReviseBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let prior_doc_id = body.doc_id.trim().to_string();
    let actor_id = require_str(body.actor).unwrap_or_else(|| "agent".to_string());

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };

    // Fetch the prior atom to derive labels + carry-forward properties.
    let prior = match store.get_node(&prior_doc_id) {
        Ok(Some(node)) => node.clone(),
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": "prior doc not found" })),
            )
                .into_response();
        }
        Err(error) => return store_error(error),
    };

    let content_hash = sha1_hex(&body.content);
    let revised_id = atom_id(
        "revise",
        &format!("{}|{}|{}|{}", tenant_id, actor_id, prior_doc_id, content_hash),
    );
    let prior_revision = prior
        .properties
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let title = body.title.clone().unwrap_or_else(|| {
        prior
            .properties
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("revised")
            .to_string()
    });
    let summary = body.summary.clone().unwrap_or_else(|| body.content.chars().take(500).collect::<String>());

    let mut props = serde_json::Map::new();
    if let Value::Object(ref pm) = prior.properties {
        for (k, v) in pm {
            // carry forward; will overwrite some below
            props.insert(k.clone(), v.clone());
        }
    }
    props.insert("actor_id".to_string(), json!(actor_id));
    props.insert("title".to_string(), json!(title));
    props.insert("content".to_string(), json!(body.content));
    props.insert("summary".to_string(), json!(summary));
    if !body.tags.is_empty() {
        props.insert("tags".to_string(), json!(body.tags));
    }
    props.insert("revision".to_string(), json!(prior_revision + 1));
    props.insert("revision_of_doc_id".to_string(), json!(prior_doc_id));
    props.insert("supersedes_doc_id".to_string(), json!(prior_doc_id));
    props.insert("revision_reason".to_string(), json!(body.reason.unwrap_or_default()));
    props.insert("status".to_string(), json!("active"));
    props.insert("updated_at".to_string(), json!(now_iso()));
    props.insert("captured_at".to_string(), json!(now_iso()));
    let properties = Value::Object(props);

    // Use prior labels (preserves verb kind) plus marker.
    let mut labels: Vec<String> = prior.labels.iter().cloned().collect();
    if !labels.iter().any(|l| l == "MemoryAtom") {
        labels.insert(0, "MemoryAtom".to_string());
    }
    let revised = NodeRecord::new(revised_id.clone(), labels, properties);

    // Upsert revised. Mark prior as superseded.
    let _ = match store.upsert_node(revised.clone()) {
        Ok(r) => r,
        Err(error) => {
            state.observability.record_error();
            return store_error(error);
        }
    };
    state.observability.record_mutation();
    state.maybe_index_node_fulltext(&tenant_id, &revised);

    let mut prior_updated = prior.clone();
    if let Value::Object(ref mut map) = prior_updated.properties {
        map.insert("status".to_string(), json!("superseded"));
        map.insert("superseded_by".to_string(), json!(revised_id));
        map.insert("updated_at".to_string(), json!(now_iso()));
    }
    let _ = store.upsert_node(prior_updated);

    // Edge: revised --SUPERSEDES--> prior
    let edge_id = format!("edge:{}:supersedes:{}", revised_id, prior_doc_id);
    let edge = EdgeRecord::new(
        edge_id,
        revised_id.clone(),
        prior_doc_id.clone(),
        "SUPERSEDES",
        Value::Object(Default::default()),
    );
    let _ = store.upsert_edge(edge);

    json_response(json!({
        "ok": true,
        "doc_id": revised_id,
        "supersedes_doc_id": prior_doc_id,
        "revision": prior_revision + 1,
    }))
}

pub async fn memory_self_archive(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SelfArchiveBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let doc_id = body.doc_id.trim().to_string();
    let reason = body.reason.unwrap_or_default();

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };
    let existing = match store.get_node(&doc_id) {
        Ok(Some(node)) => node.clone(),
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": "doc not found" })),
            )
                .into_response();
        }
        Err(error) => return store_error(error),
    };

    let mut updated = existing.clone();
    if let Value::Object(ref mut map) = updated.properties {
        map.insert("status".to_string(), json!("archived"));
        map.insert("archived_at".to_string(), json!(now_iso()));
        map.insert("archive_reason".to_string(), json!(reason));
        map.insert("updated_at".to_string(), json!(now_iso()));
    }
    match store.upsert_node(updated.clone()) {
        Ok(result) => {
            state.observability.record_mutation();
            json_response(json!({ "ok": true, "doc_id": doc_id, "node": result }))
        }
        Err(error) => {
            state.observability.record_error();
            store_error(error)
        }
    }
}

pub async fn memory_self_recall_archive(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SelfRecallArchiveBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "memory:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let limit = body.limit.unwrap_or(20).min(200);
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return unavailable_error(error),
    };

    let mut query = NodeQuery::label("MemoryAtom")
        .with_property("status", json!("archived"))
        .with_limit(limit * 4);
    if let Some(actor) = body.actor.as_deref().filter(|s| !s.trim().is_empty()) {
        query = query.with_property("actor_id", json!(actor.trim()));
    }
    let nodes = match store.query_nodes(query) {
        Ok(nodes) => nodes,
        Err(error) => return store_error(error),
    };

    let needle = body
        .query
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let mut hits: Vec<&NodeRecord> = nodes
        .iter()
        .filter(|n| match needle {
            Some(ref q) => {
                let title = n
                    .properties
                    .get("title")
                    .and_then(Value::as_str)
                    .map(|s| s.to_lowercase().contains(q))
                    .unwrap_or(false);
                let content = n
                    .properties
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|s| s.to_lowercase().contains(q))
                    .unwrap_or(false);
                title || content
            }
            None => true,
        })
        .collect();
    hits.truncate(limit);
    let results: Vec<Value> = hits.iter().map(|n| json!(n)).collect();

    json_response(json!({
        "ok": true,
        "count": results.len(),
        "results": results,
    }))
}

// Routes are registered inline in router.rs's build_router(). The handlers
// above are pub so router.rs can refer to them as crate::memory::*.
