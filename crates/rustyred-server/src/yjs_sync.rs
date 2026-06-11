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
//! ```
//!
//! Persistence: each room's full doc state is written as a `YjsDoc` node in
//! the tenant graph store (`yjs:doc:<doc_id>`, base64 update v1) after every
//! applied push, and loaded when a room first opens. Write-per-push is
//! deliberate v1 simplicity at organizer-edit rates; batch/debounce is the
//! production-hardening follow-up.
//!
//! Auth: the route is browser-facing (the public planning workspace), and
//! browser WebSockets cannot send Authorization headers. It ships
//! unauthenticated, matching the planner's current no-login posture; the
//! Phase 7 deployment gate owns the auth decision (query-param token is the
//! standard retrofit).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use rustyred_core::NodeRecord;
use serde_json::json;
use tokio::sync::{broadcast, Mutex as TokioMutex};
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::state::AppState;

/// Frame tags (first byte of every binary frame).
const TAG_PULL: u8 = 0x00;
const TAG_PULL_REPLY: u8 = 0x01;
const TAG_UPDATE: u8 = 0x02;

/// Broadcast payloads carry the origin connection id so the pusher does not
/// receive its own update back.
type RoomBroadcast = (u64, Arc<Vec<u8>>);

struct YjsRoom {
    /// `tenant:doc` key, also the broadcast identity in logs.
    key: String,
    storage_node_id: String,
    doc: TokioMutex<Doc>,
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
        tracing::warn!(node_id, "yjs: persisted state is not valid base64; starting empty");
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

/// Persist a room's full current state as a YjsDoc node.
fn save_doc(state: &AppState, tenant_id: &str, room: &YjsRoom, doc: &Doc) {
    let bytes = doc
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
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
            "updated_at_ms": now_ms(),
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
    let (tx, _) = broadcast::channel(256);
    let room = Arc::new(YjsRoom {
        key: key.clone(),
        storage_node_id: node_id,
        doc: TokioMutex::new(doc),
        tx,
    });
    registry.insert(key, Arc::clone(&room));
    room
}

/// GET /v1/tenants/:tenant_id/sync/yjs/:doc_id (WebSocket upgrade).
pub async fn yjs_sync_ws(
    Path((tenant_id, doc_id)): Path<(String, String)>,
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(state, tenant_id, doc_id, socket))
}

async fn handle_socket(state: AppState, tenant_id: String, doc_id: String, socket: WebSocket) {
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
                            handle_frame(&state, &tenant_id, &room, conn_id, &frame).await
                        {
                            if sink.send(Message::Binary(reply)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Ping(payload) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            break;
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
                        let mut txn = doc.transact_mut();
                        match txn.apply_update(update) {
                            Ok(()) => {
                                drop(txn);
                                save_doc(state, tenant_id, room, &doc);
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
            }
            None
        }
        other => {
            tracing::warn!(room = %room.key, tag = other, "yjs: unknown frame tag");
            None
        }
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
        let content = text.get_string(&txn);
        content
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
}
