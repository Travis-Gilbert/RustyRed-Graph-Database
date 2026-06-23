//! Yjs sync endpoint: RustyRed speaks the Yjs protocol via yrs (y-crdt).
//!
//! Phase 1 of the Civic Atlas event-planning build plan. BlockSuite clients
//! sync collaborative documents through this endpoint; the civic-object
//! planning store (`civic:porchfest-2026`) is the first consumer. The
//! frontend half is `RustyRedDocSource` in the Open-Flint-Atlas civic-editor
//! bundle, and the shared contract lives in the planner folder's
//! SCHEMA-CONTRACT.md.
//!
//! Wire protocol, one WebSocket per (tenant, doc), binary frames tagged by
//! their first byte. Deliberately smaller than the full y-websocket
//! protocol; it maps 1:1 onto BlockSuite's DocSource semantics:
//!
//! ```text
//! C->S  0x00 <state-vector v1>   pull handshake: client announces what it has
//! S->C  0x01 <update v1>         pull reply: server diff since that vector
//! C->S  0x02 <update v1>         push: applied to the server doc, then
//! S->C  0x02 <update v1>         broadcast to every OTHER subscriber
//! C->S  0x03 <awareness v1>      presence (cursor/selection): broadcast-only
//! S->C  0x03 <awareness v1>      relayed to every OTHER subscriber, NEVER stored
//! ```
//!
//! Consumers (SPEC-RUSTYRED-CRDT Part 5), beyond the civic planner:
//!
//! - **Code file buffers.** A file in a head's footprint opens as a doc with
//!   `doc_id = code:<repo>:<path>`. Heads edit through the doc; it is the live
//!   working buffer that converges keystroke-by-keystroke for liveness and
//!   steering. Git stays the commit and merge authority: the buffer is NOT the
//!   source of truth for committed code (the CodeCRDT evaluation, arXiv
//!   2510.18893, shows CRDT-as-authority inflates code volume and drops
//!   quality). `code:` docs are generic yjs docs here and are never civic-
//!   projected (the projection guards on the `civic:` prefix).
//! - **Per-span provenance.** Every yrs item natively carries the `client_id`
//!   that authored it. A `ProvenanceSidecar` maps each `client_id` to the actor
//!   that pushed it, so blame exists at the substrate before any git commit. It
//!   is persisted alongside `state_b64` and survives a restart.
//! - **Awareness.** Cursor/selection presence rides frame tag `0x03`. It is
//!   broadcast-only and ephemeral: never applied to the doc, never persisted.
//!   This is what lets a human watch and steer a head live in the editor.
//!
//! Persistence: each room's full doc state is written as a `YjsDoc` node in
//! the tenant graph store (`yjs:doc:<doc_id>`, base64 update v1) after every
//! applied push, with the provenance sidecar stored on the same node, and
//! loaded when a room first opens. Write-per-push is deliberate v1 simplicity
//! at organizer-edit rates; batch/debounce is the production-hardening
//! follow-up.
//!
//! Auth: the route is browser-facing (the public planning workspace), and
//! browser WebSockets cannot send Authorization headers. It ships
//! unauthenticated, matching the planner's current no-login posture; the
//! Phase 7 deployment gate owns the auth decision (query-param token is the
//! standard retrofit).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    response::IntoResponse,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use rustyred_core::NodeRecord;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{broadcast, Mutex as TokioMutex};
use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Transact, Update};

use crate::state::AppState;

/// Frame tags (first byte of every binary frame).
const TAG_PULL: u8 = 0x00;
const TAG_PULL_REPLY: u8 = 0x01;
const TAG_UPDATE: u8 = 0x02;
const TAG_AWARENESS: u8 = 0x03;

/// Broadcast payloads carry the origin connection id so the pusher does not
/// receive its own update back.
type RoomBroadcast = (u64, Arc<Vec<u8>>);

#[derive(Debug, Default, Deserialize)]
pub struct YjsSyncQuery {
    #[serde(default)]
    actor: String,
}

pub(crate) struct YjsRoom {
    /// `tenant:doc` key, also the broadcast identity in logs.
    pub(crate) key: String,
    /// Raw doc id; the civic projection matches on its `civic:` prefix.
    pub(crate) doc_id: String,
    storage_node_id: String,
    pub(crate) doc: TokioMutex<Doc>,
    /// Per-span authorship (client_id -> actor), persisted with the doc.
    pub(crate) provenance: StdMutex<ProvenanceSidecar>,
    tx: broadcast::Sender<RoomBroadcast>,
}

/// Process-global room registry keyed by `tenant:doc`. Rooms are tiny
/// (a yrs Doc plus a channel); eviction is unnecessary at planner scale.
static ROOMS: OnceLock<TokioMutex<BTreeMap<String, Arc<YjsRoom>>>> = OnceLock::new();
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

fn rooms() -> &'static TokioMutex<BTreeMap<String, Arc<YjsRoom>>> {
    ROOMS.get_or_init(|| TokioMutex::new(BTreeMap::new()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn storage_node_id(doc_id: &str) -> String {
    format!("yjs:doc:{doc_id}")
}

/// Load a doc's persisted state from the tenant store into a fresh yrs Doc.
fn load_doc(state: &AppState, tenant_id: &str, node_id: &str) -> Doc {
    let doc = Doc::new();
    let Ok(store) = state.tenant_graph_store(tenant_id) else {
        return doc;
    };
    let Ok(Some(node)) = store.get_node(node_id) else {
        return doc;
    };
    let Some(state_b64) = node.properties.get("state_b64").and_then(|v| v.as_str()) else {
        return doc;
    };
    let Ok(bytes) = BASE64.decode(state_b64) else {
        tracing::warn!(
            node_id,
            "yjs: persisted state is not valid base64; starting empty"
        );
        return doc;
    };
    match Update::decode_v1(&bytes) {
        Ok(update) => {
            let mut txn = doc.transact_mut();
            if let Err(error) = txn.apply_update(update) {
                tracing::warn!(node_id, %error, "yjs: persisted update failed to apply; starting empty");
            }
        }
        Err(error) => {
            tracing::warn!(node_id, %error, "yjs: persisted update failed to decode; starting empty");
        }
    }
    doc
}

/// Load a doc's persisted per-span provenance sidecar from the YjsDoc node.
fn load_provenance(state: &AppState, tenant_id: &str, node_id: &str) -> ProvenanceSidecar {
    let Ok(store) = state.tenant_graph_store(tenant_id) else {
        return ProvenanceSidecar::default();
    };
    let Ok(Some(node)) = store.get_node(node_id) else {
        return ProvenanceSidecar::default();
    };
    node.properties
        .get("provenance")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// Persist a room's full current state as a YjsDoc node.
fn save_doc(state: &AppState, tenant_id: &str, room: &YjsRoom, doc: &Doc, actor: &str) {
    // `get_or_insert_text` opens its own mutable transaction internally, so the
    // root type must be resolved BEFORE the read transaction below. Resolving it
    // while `txn` is held deadlocks: the mutable transact blocks on the read
    // transaction that never releases (yrs transactions are mutually exclusive).
    let text = doc.get_or_insert_text("t");
    let txn = doc.transact();
    let bytes = txn.encode_state_as_update_v1(&StateVector::default());
    let text_len = text.get_string(&txn).chars().count();
    drop(txn);
    let updated_at_ms = now_ms();
    let mut store = match state.tenant_graph_store(tenant_id) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(room = %room.key, ?error, "yjs: store unavailable; update held in memory only");
            return;
        }
    };
    let node = NodeRecord::new(
        room.storage_node_id.clone(),
        ["YjsDoc"],
        json!({
            "doc_key": room.key,
            "state_b64": BASE64.encode(&bytes),
            "byte_len": bytes.len(),
            "updated_at_ms": updated_at_ms,
            "actor_attribution": attribution_sidecar(actor, text_len, updated_at_ms),
            "provenance": room
                .provenance
                .lock()
                .ok()
                .map(|sidecar| serde_json::to_value(&*sidecar).unwrap_or_else(|_| json!({})))
                .unwrap_or_else(|| json!({})),
        }),
    );
    if let Err(error) = store.upsert_node(node) {
        tracing::warn!(room = %room.key, ?error, "yjs: persist failed; update held in memory only");
    }
}

async fn get_or_open_room(state: &AppState, tenant_id: &str, doc_id: &str) -> Arc<YjsRoom> {
    let key = format!("{tenant_id}:{doc_id}");
    let mut registry = rooms().lock().await;
    if let Some(room) = registry.get(&key) {
        return Arc::clone(room);
    }
    let node_id = storage_node_id(doc_id);
    let doc = load_doc(state, tenant_id, &node_id);
    let provenance = load_provenance(state, tenant_id, &node_id);
    let (tx, _) = broadcast::channel(256);
    let room = Arc::new(YjsRoom {
        key: key.clone(),
        doc_id: doc_id.to_string(),
        storage_node_id: node_id,
        doc: TokioMutex::new(doc),
        provenance: StdMutex::new(provenance),
        tx,
    });
    registry.insert(key, Arc::clone(&room));
    // Prime the civic projection on room open (not only on push): after a
    // server restart the in-memory geometry designation is gone, and a
    // quiescent doc would otherwise leave geometry queries empty until the
    // next organizer edit. The first client connection re-projects from the
    // loaded doc, which re-registers the designation and backfills indexes.
    crate::civic_projection::schedule_civic_projection(state, tenant_id, &room);
    room
}

/// GET /v1/tenants/:tenant_id/sync/yjs/:doc_id (WebSocket upgrade).
pub async fn yjs_sync_ws(
    Path((tenant_id, doc_id)): Path<(String, String)>,
    Query(query): Query<YjsSyncQuery>,
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(state, tenant_id, doc_id, query.actor, socket))
}

async fn handle_socket(
    state: AppState,
    tenant_id: String,
    doc_id: String,
    actor: String,
    socket: WebSocket,
) {
    let room = get_or_open_room(&state, &tenant_id, &doc_id).await;
    let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
    let mut rx = room.tx.subscribe();
    let (mut sink, mut stream) = socket.split();
    tracing::debug!(room = %room.key, conn_id, "yjs: subscriber connected");

    loop {
        tokio::select! {
            inbound = stream.next() => {
                let Some(Ok(message)) = inbound else { break };
                match message {
                    Message::Binary(frame) => {
                        if let Some(reply) =
                            handle_frame(&state, &tenant_id, &room, conn_id, &actor, &frame).await
                        {
                            if sink.send(Message::Binary(reply)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Ping(payload) => {
                        match sink.send(Message::Pong(payload)).await {
                            Ok(()) => {}
                            Err(_) => break,
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            peer = rx.recv() => {
                match peer {
                    Ok((origin, bytes)) if origin != conn_id => {
                        if sink.send(Message::Binary(bytes.as_ref().clone())).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        // A lagged subscriber re-converges with one pull.
                        tracing::warn!(room = %room.key, conn_id, skipped, "yjs: subscriber lagged");
                        let doc = room.doc.lock().await;
                        let full = doc
                            .transact()
                            .encode_state_as_update_v1(&StateVector::default());
                        drop(doc);
                        let mut frame = Vec::with_capacity(full.len() + 1);
                        frame.push(TAG_UPDATE);
                        frame.extend_from_slice(&full);
                        if sink.send(Message::Binary(frame)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    tracing::debug!(room = %room.key, conn_id, "yjs: subscriber disconnected");
}

/// Apply one inbound frame; returns the direct reply frame, if any.
async fn handle_frame(
    state: &AppState,
    tenant_id: &str,
    room: &Arc<YjsRoom>,
    conn_id: u64,
    actor: &str,
    frame: &[u8],
) -> Option<Vec<u8>> {
    let (&tag, payload) = frame.split_first()?;
    // yrs Update/StateVector values are !Send (Rc internals), so they are
    // always created AFTER the room lock's await and never span one.
    match tag {
        TAG_PULL => {
            let doc = room.doc.lock().await;
            let sv = match StateVector::decode_v1(payload) {
                Ok(sv) => sv,
                Err(error) => {
                    tracing::warn!(room = %room.key, %error, "yjs: bad state vector in pull");
                    return None;
                }
            };
            let diff = doc.transact().encode_state_as_update_v1(&sv);
            let mut reply = Vec::with_capacity(diff.len() + 1);
            reply.push(TAG_PULL_REPLY);
            reply.extend_from_slice(&diff);
            Some(reply)
        }
        TAG_UPDATE => {
            let applied = {
                let doc = room.doc.lock().await;
                match Update::decode_v1(payload) {
                    Ok(update) => {
                        // SPEC Part 5 provenance: the before/after state-vector
                        // diff names exactly the client_ids this update advanced;
                        // attribute them to the pushing actor.
                        let before = doc.transact().state_vector();
                        let mut txn = doc.transact_mut();
                        match txn.apply_update(update) {
                            Ok(()) => {
                                drop(txn);
                                let after = doc.transact().state_vector();
                                if let Ok(mut provenance) = room.provenance.lock() {
                                    provenance.observe(&before, &after, actor, now_ms());
                                }
                                save_doc(state, tenant_id, room, &doc, actor);
                                true
                            }
                            Err(error) => {
                                tracing::warn!(room = %room.key, %error, "yjs: update failed to apply");
                                false
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(room = %room.key, %error, "yjs: bad update in push");
                        false
                    }
                }
            };
            if applied {
                let _ = room.tx.send((conn_id, Arc::new(frame.to_vec())));
                // Civic docs mirror into the tenant graph. The projection is
                // debounced and best-effort: it never blocks or fails sync.
                crate::civic_projection::schedule_civic_projection(state, tenant_id, room);
            }
            None
        }
        TAG_AWARENESS => {
            let _ = room.tx.send((conn_id, Arc::new(frame.to_vec())));
            None
        }
        other => {
            tracing::warn!(room = %room.key, tag = other, "yjs: unknown frame tag");
            None
        }
    }
}

fn attribution_sidecar(actor: &str, text_len: usize, updated_at_ms: u64) -> serde_json::Value {
    let actor = actor.trim();
    if actor.is_empty() || actor == "unknown" {
        return json!([]);
    }
    json!([
        {
            "actor": actor,
            "start": 0,
            "end": text_len,
            "updated_at_ms": updated_at_ms,
        }
    ])
}

/// Per-span authorship for a doc (SPEC-RUSTYRED-CRDT Part 5, faithful refinement
/// of the coarse `attribution_sidecar`). Every yrs item natively carries the
/// `client_id` of its author; this sidecar maps that `client_id` to the actor
/// that pushed the update introducing it, so blame exists at the substrate
/// before any git commit. Persisted alongside `state_b64` on the `YjsDoc` node
/// and reloaded on room open, so a known span still resolves to its writer
/// after a restart.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ProvenanceSidecar {
    /// yrs `client_id` -> attribution.
    #[serde(default)]
    pub(crate) clients: BTreeMap<u64, ActorAttribution>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ActorAttribution {
    pub(crate) actor: String,
    pub(crate) first_seen_ms: u64,
    pub(crate) last_seen_ms: u64,
    pub(crate) updates: u64,
}

impl ProvenanceSidecar {
    /// Record that `actor` pushed an update whose effect advanced these clients'
    /// clocks - i.e. the `client_id`s that authored the update's new items (the
    /// before/after state-vector diff bounds exactly which clients contributed).
    /// The first actor to introduce a `client_id` owns it (a head gets a stable
    /// yrs client id); later updates from the same client bump activity only.
    fn observe(&mut self, before: &StateVector, after: &StateVector, actor: &str, now_ms: u64) {
        for (client, after_clock) in after.iter() {
            if *after_clock > before.get(client) {
                let entry = self
                    .clients
                    .entry(client.get())
                    .or_insert_with(|| ActorAttribution {
                        actor: actor.to_string(),
                        first_seen_ms: now_ms,
                        last_seen_ms: now_ms,
                        updates: 0,
                    });
                entry.last_seen_ms = now_ms;
                entry.updates += 1;
            }
        }
    }

    /// Blame: which actor authored the item carrying this yrs `client_id`.
    #[cfg(test)]
    pub(crate) fn writer_of(&self, client_id: u64) -> Option<&str> {
        self.clients.get(&client_id).map(|a| a.actor.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{GetString, Text};

    fn text_update(content: &str) -> Vec<u8> {
        let doc = Doc::new();
        let text = doc.get_or_insert_text("t");
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 0, content);
        drop(txn);
        let update = doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        update
    }

    fn doc_text(doc: &Doc) -> String {
        let text = doc.get_or_insert_text("t");
        let txn = doc.transact();
        text.get_string(&txn)
    }

    #[test]
    fn pull_diff_and_push_converge_two_docs() {
        // Server room doc receives a push from A; B pulls with an empty
        // state vector and applies the diff; both read the same text.
        let server = Doc::new();
        {
            let update = Update::decode_v1(&text_update("porch")).unwrap();
            let mut txn = server.transact_mut();
            txn.apply_update(update).unwrap();
        }
        let diff = server
            .transact()
            .encode_state_as_update_v1(&StateVector::default());

        let client_b = Doc::new();
        {
            let mut txn = client_b.transact_mut();
            txn.apply_update(Update::decode_v1(&diff).unwrap()).unwrap();
        }
        assert_eq!(doc_text(&client_b), "porch");
    }

    #[test]
    fn incremental_pull_returns_only_missing_state() {
        // After B has synced once, a second pull with B's state vector
        // returns a diff that still applies cleanly and converges.
        let server = Doc::new();
        {
            let mut txn = server.transact_mut();
            txn.apply_update(Update::decode_v1(&text_update("flint ")).unwrap())
                .unwrap();
        }
        let client_b = Doc::new();
        {
            let diff = server
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            let mut txn = client_b.transact_mut();
            txn.apply_update(Update::decode_v1(&diff).unwrap()).unwrap();
        }
        // Server advances (a second writer).
        {
            let text = server.get_or_insert_text("t");
            let mut txn = server.transact_mut();
            let len = text.get_string(&txn).len() as u32;
            text.insert(&mut txn, len, "porchfest");
        }
        let sv_b = client_b.transact().state_vector();
        let diff = server.transact().encode_state_as_update_v1(&sv_b);
        {
            let mut txn = client_b.transact_mut();
            txn.apply_update(Update::decode_v1(&diff).unwrap()).unwrap();
        }
        assert_eq!(doc_text(&client_b), "flint porchfest");
    }

    #[test]
    fn frame_round_trip_preserves_tag_and_payload() {
        let payload = text_update("x");
        let mut frame = Vec::with_capacity(payload.len() + 1);
        frame.push(TAG_UPDATE);
        frame.extend_from_slice(&payload);
        let (&tag, body) = frame.split_first().unwrap();
        assert_eq!(tag, TAG_UPDATE);
        assert_eq!(body, payload.as_slice());
        assert!(Update::decode_v1(body).is_ok());
    }

    #[test]
    fn attribution_sidecar_maps_full_text_span_to_actor() {
        let sidecar = attribution_sidecar("codex", 5, 42);

        assert_eq!(sidecar[0]["actor"].as_str(), Some("codex"));
        assert_eq!(sidecar[0]["start"].as_u64(), Some(0));
        assert_eq!(sidecar[0]["end"].as_u64(), Some(5));
    }

    #[test]
    fn awareness_tag_is_not_a_persisted_update_tag() {
        assert_ne!(TAG_AWARENESS, TAG_UPDATE);
        assert_eq!(TAG_AWARENESS, 0x03);
    }

    #[test]
    fn provenance_resolves_spans_to_writers_and_survives_reload() {
        // SPEC Part 5 A5.2: per-span provenance - a known span resolves to its
        // writer, and the mapping survives a restart (serde round-trip is the
        // persist-to-node-then-reload path). Two writers on distinct yrs client
        // ids; each item natively carries its author's client id, so the
        // before/after state-vector diff attributes the span to the right actor.
        let server = Doc::new();
        let mut sidecar = ProvenanceSidecar::default();

        let mut apply_as = |doc_client: u64, actor: &str, span: &str| {
            let writer = Doc::with_client_id(doc_client);
            {
                let text = writer.get_or_insert_text("t");
                let mut txn = writer.transact_mut();
                text.insert(&mut txn, 0, span);
            }
            let update = writer
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            let before = server.transact().state_vector();
            {
                let mut txn = server.transact_mut();
                txn.apply_update(Update::decode_v1(&update).unwrap())
                    .unwrap();
            }
            let after = server.transact().state_vector();
            sidecar.observe(&before, &after, actor, doc_client);
        };

        apply_as(101, "alice", "alice-span");
        apply_as(202, "bob", "bob-span");

        assert_eq!(sidecar.writer_of(101), Some("alice"));
        assert_eq!(sidecar.writer_of(202), Some("bob"));

        // Survives restart: serialize as persisted on the YjsDoc node, reload.
        let persisted = serde_json::to_value(&sidecar).unwrap();
        let reloaded: ProvenanceSidecar = serde_json::from_value(persisted).unwrap();
        assert_eq!(reloaded.writer_of(101), Some("alice"));
        assert_eq!(reloaded.writer_of(202), Some("bob"));
    }

    #[tokio::test]
    async fn awareness_traffic_does_not_persist_to_yjs_node() {
        // SPEC Part 5 A5.3: awareness (0x03) frames broadcast to subscribers but
        // are NEVER written to the YjsDoc node. Apply an UPDATE (which persists),
        // snapshot the persisted state_b64, push an AWARENESS frame through the
        // real handler, and assert the persisted node is byte-identical.
        use crate::config::{Config, StorageMode};

        let config = Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Memory,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: rustyred_core::RedCoreDurability::None,
            snapshot_interval_writes: 0,
            strict_acid: false,
            concurrency: "single_writer".to_string(),
            txn_isolation: "snapshot".to_string(),
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
            tenant_config_overrides: Default::default(),
            tenant_config_error: None,
            slow_query_threshold_nanos: 100_000_000,
            slow_query_capacity: 128,
            slow_query_log: None,
            hybrid_scoring: rustyred_core::HybridScoringConfig::default(),
            redis_url: "not-a-redis-url".to_string(),
            redis_key_prefix: "rusty-red".to_string(),
            require_auth: false,
            allowed_origins: Vec::new(),
            api_tokens: Vec::new(),
            service_name: "rusty-red".to_string(),
            api_title: "Rusty Red".to_string(),
            public_url: None,
            federate: true,
            federate_hub_url: None,
            federate_token: None,
            federate_peer_id: None,
            federate_private_key: None,
            federate_provenance: false,
            federate_snapshot_text_bytes: rustyred_search::DEFAULT_WEB_COMMONS_SNAPSHOT_TEXT_BYTES,
            mcp_enabled: true,
            mcp_read_only: true,
            mcp_allow_admin: false,
            mcp_default_tenant: "default".to_string(),
        };
        let state = AppState::new(config);
        let tenant = "default";
        let doc_id = "code:repo:lib.rs";
        let room = get_or_open_room(&state, tenant, doc_id).await;
        let node_id = storage_node_id(doc_id);

        let read_state_b64 = |state: &AppState| {
            state
                .tenant_graph_store(tenant)
                .ok()
                .and_then(|store| store.get_node(&node_id).ok().flatten())
                .and_then(|node| {
                    node.properties
                        .get("state_b64")
                        .and_then(|v| v.as_str().map(String::from))
                })
        };

        // Apply an UPDATE -> persists the YjsDoc node.
        let mut update = vec![TAG_UPDATE];
        update.extend_from_slice(&text_update("hello"));
        handle_frame(&state, tenant, &room, 1, "alice", &update).await;
        let persisted_before = read_state_b64(&state);
        assert!(persisted_before.is_some(), "update should persist the node");

        // Push an AWARENESS frame -> broadcast-only, must NOT persist.
        let awareness = vec![TAG_AWARENESS, 0x01, 0x02, 0x03];
        let reply = handle_frame(&state, tenant, &room, 2, "alice", &awareness).await;
        assert!(reply.is_none(), "awareness produces no direct reply");

        let persisted_after = read_state_b64(&state);
        assert_eq!(
            persisted_before, persisted_after,
            "awareness traffic must not change the persisted YjsDoc node"
        );
    }
}
