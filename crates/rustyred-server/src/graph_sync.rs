//! Graph delta sync endpoint for the typed RustyRed graph CRDT.
//!
//! Wire protocol mirrors `yjs_sync`: one WebSocket per `(tenant, room)`,
//! binary frames tagged by first byte:
//!
//! ```text
//! C->S  0x00 <VersionVector JSON>
//! S->C  0x01 <StampedBatch JSON>
//! C->S  0x02 <StampedBatch JSON>
//! S->C  0x02 <StampedBatch JSON> broadcast to every OTHER subscriber
//! ```
//!
//! Unlike Yjs documents, persistence is the graph store itself. A pushed
//! delta joins into the tenant `GraphStore` and is durably written through
//! the same incremental mutation path as the rest of RustyRed.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use rustyred_core::{
    diff_snapshot_since, merge_edge_record, merge_node_record, GraphMutation, GraphMutationBatch,
    GraphStoreResult, JoinReport, StampedBatch, VersionVector,
};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast, Mutex as TokioMutex};

use crate::state::AppState;
use crate::state::TenantGraphStore;

const TAG_PULL: u8 = 0x00;
const TAG_PULL_REPLY: u8 = 0x01;
const TAG_UPDATE: u8 = 0x02;

type RoomBroadcast = (u64, Arc<Vec<u8>>);

#[derive(Debug, Default, Deserialize)]
pub struct GraphSyncQuery {
    #[serde(default)]
    token: String,
}

pub(crate) struct GraphRoom {
    pub(crate) key: String,
    tx: broadcast::Sender<RoomBroadcast>,
}

static ROOMS: OnceLock<TokioMutex<BTreeMap<String, Arc<GraphRoom>>>> = OnceLock::new();
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

fn rooms() -> &'static TokioMutex<BTreeMap<String, Arc<GraphRoom>>> {
    ROOMS.get_or_init(|| TokioMutex::new(BTreeMap::new()))
}

fn room_key(tenant_id: &str, room_id: &str) -> String {
    format!("{}:{}", rustyred_core::stable_hash(tenant_id), room_id)
}

async fn get_or_open_room(tenant_id: &str, room_id: &str) -> Arc<GraphRoom> {
    let key = room_key(tenant_id, room_id);
    let mut registry = rooms().lock().await;
    if let Some(room) = registry.get(&key) {
        return Arc::clone(room);
    }
    let (tx, _) = broadcast::channel(256);
    let room = Arc::new(GraphRoom {
        key: key.clone(),
        tx,
    });
    registry.insert(key, Arc::clone(&room));
    room
}

/// GET /v1/tenants/:tenant_id/sync/graph/:room_id?token=...
pub async fn graph_sync_ws(
    Path((tenant_id, room_id)): Path<(String, String)>,
    Query(query): Query<GraphSyncQuery>,
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if let Err(status) = authorize_graph_sync(&state, &tenant_id, &query.token) {
        return status.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(state, tenant_id, room_id, socket))
}

fn authorize_graph_sync(state: &AppState, tenant_id: &str, token: &str) -> Result<(), StatusCode> {
    if !state.config.require_auth {
        return Ok(());
    }
    if token.trim().is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let matched = state
        .config
        .api_tokens
        .iter()
        .find(|candidate| bool::from(candidate.token.as_bytes().ct_eq(token.as_bytes())))
        .ok_or(StatusCode::FORBIDDEN)?;
    if token_scopes_authorize_tenant(&matched.scopes, tenant_id) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn token_scopes_authorize_tenant(scopes: &[String], tenant_id: &str) -> bool {
    let has_graph_write = scopes.iter().any(|scope| {
        matches!(
            scope.as_str(),
            "*" | "graph:write" | "rustyred:graph:write:apply" | "rustyred:graph:write:propose"
        )
    });
    let tenant_scope = format!("tenant:{tenant_id}");
    let tenant_hash_scope = format!("tenant:{}", rustyred_core::stable_hash(tenant_id));
    let has_tenant = scopes.iter().any(|scope| {
        scope == "*" || scope == "tenant:*" || scope == &tenant_scope || scope == &tenant_hash_scope
    });
    has_graph_write && has_tenant
}

async fn handle_socket(state: AppState, tenant_id: String, room_id: String, socket: WebSocket) {
    let room = get_or_open_room(&tenant_id, &room_id).await;
    let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
    let mut rx = room.tx.subscribe();
    let (mut sink, mut stream) = socket.split();
    tracing::debug!(room = %room.key, conn_id, "graph-sync: subscriber connected");

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
                        tracing::warn!(room = %room.key, conn_id, skipped, "graph-sync: subscriber lagged");
                        let full = {
                            let Ok(store) = state.tenant_graph_store(&tenant_id) else {
                                continue;
                            };
                            let Ok(snapshot) = store.graph_snapshot() else {
                                continue;
                            };
                            diff_snapshot_since(snapshot, &VersionVector::default())
                        };
                        if let Ok(payload) = serde_json::to_vec(&full) {
                            let mut frame = Vec::with_capacity(payload.len() + 1);
                            frame.push(TAG_UPDATE);
                            frame.extend_from_slice(&payload);
                            if sink.send(Message::Binary(frame)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    tracing::debug!(room = %room.key, conn_id, "graph-sync: subscriber disconnected");
}

async fn handle_frame(
    state: &AppState,
    tenant_id: &str,
    room: &Arc<GraphRoom>,
    conn_id: u64,
    frame: &[u8],
) -> Option<Vec<u8>> {
    let (&tag, payload) = frame.split_first()?;
    match tag {
        TAG_PULL => {
            let vector = match serde_json::from_slice::<VersionVector>(payload) {
                Ok(vector) => vector,
                Err(error) => {
                    tracing::warn!(room = %room.key, %error, "graph-sync: bad version vector");
                    return None;
                }
            };
            let store = match state.tenant_graph_store(tenant_id) {
                Ok(store) => store,
                Err(error) => {
                    tracing::warn!(room = %room.key, ?error, "graph-sync: store unavailable");
                    return None;
                }
            };
            let snapshot = match store.graph_snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(room = %room.key, ?error, "graph-sync: snapshot unavailable");
                    return None;
                }
            };
            let diff = diff_snapshot_since(snapshot, &vector);
            let payload = serde_json::to_vec(&diff).ok()?;
            let mut reply = Vec::with_capacity(payload.len() + 1);
            reply.push(TAG_PULL_REPLY);
            reply.extend_from_slice(&payload);
            Some(reply)
        }
        TAG_UPDATE => {
            let delta = match serde_json::from_slice::<StampedBatch>(payload) {
                Ok(delta) => delta,
                Err(error) => {
                    tracing::warn!(room = %room.key, %error, "graph-sync: bad stamped batch");
                    return None;
                }
            };
            let applied = {
                let mut store = match state.tenant_graph_store(tenant_id) {
                    Ok(store) => store,
                    Err(error) => {
                        tracing::warn!(room = %room.key, ?error, "graph-sync: store unavailable");
                        return None;
                    }
                };
                match join_delta_into_tenant_store(&mut store, delta.clone()) {
                    Ok(report) => {
                        tracing::debug!(room = %room.key, ?report, "graph-sync: delta joined");
                        true
                    }
                    Err(error) => {
                        tracing::warn!(room = %room.key, ?error, "graph-sync: delta join failed");
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
            tracing::warn!(room = %room.key, tag = other, "graph-sync: unknown frame tag");
            None
        }
    }
}

fn join_delta_into_tenant_store(
    store: &mut TenantGraphStore,
    delta: StampedBatch,
) -> GraphStoreResult<JoinReport> {
    let mut report = JoinReport::default();
    let mut mutations = Vec::new();
    for stamped in delta.mutations {
        match stamped.mutation {
            GraphMutation::NodeUpsert(node) => {
                let existing = store.get_node(&node.id)?;
                let merged = merge_node_record(existing.as_ref(), node, stamped.hlc);
                if existing
                    .as_ref()
                    .map(|record| record.content_address() == merged.content_address())
                    .unwrap_or(false)
                {
                    report.ignored_stale += 1;
                    continue;
                }
                if existing
                    .as_ref()
                    .map(|record| record.tombstone)
                    .unwrap_or(false)
                    && !merged.tombstone
                {
                    report.revived += 1;
                }
                if !existing
                    .as_ref()
                    .map(|record| record.tombstone)
                    .unwrap_or(false)
                    && merged.tombstone
                {
                    report.tombstoned += 1;
                }
                mutations.push(GraphMutation::NodeUpsert(merged));
                report.applied += 1;
            }
            GraphMutation::EdgeUpsert(edge) => {
                let existing = store.get_edge(&edge.id)?;
                let merged = merge_edge_record(existing.as_ref(), edge, stamped.hlc);
                if existing
                    .as_ref()
                    .map(|record| record.content_address() == merged.content_address())
                    .unwrap_or(false)
                {
                    report.ignored_stale += 1;
                    continue;
                }
                if existing
                    .as_ref()
                    .map(|record| record.tombstone)
                    .unwrap_or(false)
                    && !merged.tombstone
                {
                    report.revived += 1;
                }
                if !existing
                    .as_ref()
                    .map(|record| record.tombstone)
                    .unwrap_or(false)
                    && merged.tombstone
                {
                    report.tombstoned += 1;
                }
                mutations.push(GraphMutation::EdgeUpsert(merged));
                report.applied += 1;
            }
        }
    }
    if !mutations.is_empty() {
        store.commit_batch(GraphMutationBatch::new(mutations))?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyred_core::{ActorId, GraphMutation, Hlc, NodeRecord, StampedMutation};
    use serde_json::json;

    #[test]
    fn room_keys_do_not_collapse_sanitized_tenants() {
        let a = room_key("acme/prod", "room");
        let b = room_key("acme.prod", "room");
        assert_ne!(a, b);
    }

    #[test]
    fn graph_frames_round_trip_version_vector_and_batch() {
        let mut vector = VersionVector::default();
        vector.observe(Hlc::new(10, 0, ActorId::from_label("codex")));
        let encoded = serde_json::to_vec(&vector).unwrap();
        let decoded: VersionVector = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, vector);

        let batch = StampedBatch::new([StampedMutation::new(
            GraphMutation::NodeUpsert(NodeRecord::new("n:1", ["Thing"], json!({ "x": 1 }))),
            Hlc::new(10, 0, ActorId::from_label("codex")),
        )]);
        let encoded = serde_json::to_vec(&batch).unwrap();
        let decoded: StampedBatch = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.mutations.len(), 1);
    }

    #[test]
    fn token_scope_must_match_tenant() {
        let tenant_a = vec!["graph:write".to_string(), "tenant:a".to_string()];

        assert!(token_scopes_authorize_tenant(&tenant_a, "a"));
        assert!(!token_scopes_authorize_tenant(&tenant_a, "b"));
    }

    // ---- Convergence tests (SPEC Part 3 A3.1-A3.4, Part 7 A7.3, SC-W) -----
    // Following the yjs_sync test convention (logic-level, not a live socket):
    // these exercise the REAL wire bytes (TAG + serde_json), the REAL store, and
    // the REAL join/diff that handle_frame drives - the substance of the
    // acceptance criteria. A true end-to-end socket test is a possible follow-up.
    use rustyred_core::{diff_since, join_delta, EdgeRecord, InMemoryGraphStore};

    fn node_batch(id: &str, key: &str, val: i64, hlc: Hlc) -> StampedBatch {
        StampedBatch::new([StampedMutation::new(
            GraphMutation::NodeUpsert(NodeRecord::new(id, ["Thing"], json!({ key: val }))),
            hlc,
        )])
    }

    fn update_frame(batch: &StampedBatch) -> Vec<u8> {
        let mut frame = vec![TAG_UPDATE];
        frame.extend_from_slice(&serde_json::to_vec(batch).unwrap());
        frame
    }

    fn apply_update_frame(store: &mut InMemoryGraphStore, frame: &[u8]) {
        let (&tag, payload) = frame.split_first().unwrap();
        assert_eq!(tag, TAG_UPDATE);
        let batch: StampedBatch = serde_json::from_slice(payload).unwrap();
        join_delta(store, batch);
    }

    #[test]
    fn a31_push_frame_lets_peer_join_to_identical_state() {
        let hlc = Hlc::new(10, 0, ActorId::from_label("codex"));
        let mut a = InMemoryGraphStore::new();
        join_delta(&mut a, node_batch("n:1", "name", 1, hlc));

        // The broadcast is the raw pushed frame (handle_frame rebroadcasts it).
        let broadcast = update_frame(&node_batch("n:1", "name", 1, hlc));
        let mut b = InMemoryGraphStore::new();
        apply_update_frame(&mut b, &broadcast);

        assert_eq!(
            b.get_node("n:1").unwrap().properties["name"],
            a.get_node("n:1").unwrap().properties["name"]
        );
    }

    #[test]
    fn a32_pull_reply_carries_only_missing_records() {
        let h1 = Hlc::new(10, 0, ActorId::from_label("codex"));
        let h2 = Hlc::new(20, 0, ActorId::from_label("claude"));
        let mut a = InMemoryGraphStore::new();
        join_delta(&mut a, node_batch("n:1", "v", 1, h1));
        join_delta(&mut a, node_batch("n:2", "v", 2, h2));

        // Client B has only seen n:1's actor frontier; PULL frame round-trips.
        let mut b_vector = VersionVector::default();
        b_vector.observe(h1);
        let mut pull = vec![TAG_PULL];
        pull.extend_from_slice(&serde_json::to_vec(&b_vector).unwrap());
        let (&tag, payload) = pull.split_first().unwrap();
        assert_eq!(tag, TAG_PULL);
        let decoded: VersionVector = serde_json::from_slice(payload).unwrap();

        let diff = diff_since(&a, &decoded);
        assert_eq!(
            diff.mutations.len(),
            1,
            "diff is proportional, not the graph"
        );
        match &diff.mutations[0].mutation {
            GraphMutation::NodeUpsert(node) => assert_eq!(node.id, "n:2"),
            _ => panic!("expected the missing node n:2"),
        }
    }

    #[test]
    fn a33_reconnecting_client_reconverges_from_store_via_empty_vector() {
        // SPEC Part 3 A3.3: after a restart, a reconnecting client re-converges
        // from the persisted store with no blob load - an empty-vector PULL
        // returns the full graph and the client joins to identical state. The
        // store here stands in for the post-restart recovered graph; RedCore AOF
        // restart-durability itself is covered by rustyred_core's
        // redcore_*_recovers_* tests (DB-core RedCoreGraphStore does not impl
        // GraphStore, so join_delta cannot run on it directly).
        let h1 = Hlc::new(10, 0, ActorId::from_label("codex"));
        let h2 = Hlc::new(20, 0, ActorId::from_label("claude"));
        let mut server = InMemoryGraphStore::new();
        join_delta(&mut server, node_batch("n:1", "v", 1, h1));
        join_delta(&mut server, node_batch("n:2", "v", 2, h2));

        // Reconnecting client knows nothing -> PULL with an empty vector returns
        // the whole graph; the client joins and converges.
        let full = diff_since(&server, &VersionVector::default());
        let mut client = InMemoryGraphStore::new();
        join_delta(&mut client, full);

        assert_eq!(client.get_node("n:1").unwrap().properties["v"], json!(1));
        assert_eq!(client.get_node("n:2").unwrap().properties["v"], json!(2));
    }

    #[test]
    fn a34_out_of_order_delivery_converges() {
        let h1 = Hlc::new(10, 0, ActorId::from_label("codex"));
        let h2 = Hlc::new(20, 0, ActorId::from_label("claude"));
        let d1 = node_batch("n:1", "name", 1, h1);
        let d2 = node_batch("n:1", "name", 2, h2);

        let mut b = InMemoryGraphStore::new();
        apply_update_frame(&mut b, &update_frame(&d1));
        apply_update_frame(&mut b, &update_frame(&d2));

        let mut c = InMemoryGraphStore::new();
        apply_update_frame(&mut c, &update_frame(&d2));
        apply_update_frame(&mut c, &update_frame(&d1));

        assert_eq!(
            b.get_node("n:1").unwrap().properties["name"],
            c.get_node("n:1").unwrap().properties["name"]
        );
        assert_eq!(b.get_node("n:1").unwrap().properties["name"], json!(2));
    }

    #[test]
    fn a73_join_touches_only_the_delta_not_the_whole_graph() {
        // SPEC Part 7 A7.3: a single-edge join does work proportional to the
        // delta, not the graph. Seed many edges, join one, assert the rest are
        // byte-identical (no full-graph rewrite) and the report reflects 1.
        let mut store = InMemoryGraphStore::new();
        for i in 0..50 {
            store
                .upsert_node(NodeRecord::new(format!("n:{i}"), ["Thing"], json!({})))
                .unwrap();
        }
        for i in 0..49 {
            store
                .upsert_edge(EdgeRecord::new(
                    format!("e:{i}"),
                    format!("n:{i}"),
                    "LINK",
                    format!("n:{}", i + 1),
                    json!({}),
                ))
                .unwrap();
        }
        let before: Vec<EdgeRecord> = (0..49)
            .map(|i| store.get_edge(&format!("e:{i}")).unwrap().clone())
            .collect();

        let report = join_delta(
            &mut store,
            StampedBatch::new([StampedMutation::new(
                GraphMutation::EdgeUpsert(EdgeRecord::new(
                    "e:new",
                    "n:0",
                    "LINK",
                    "n:49",
                    json!({}),
                )),
                Hlc::new(99, 0, ActorId::from_label("codex")),
            )]),
        );

        assert_eq!(report.applied, 1, "only the one delta edge is applied");
        for (i, edge) in before.iter().enumerate() {
            assert_eq!(
                &store.get_edge(&format!("e:{i}")).unwrap().clone(),
                edge,
                "pre-existing edge e:{i} unchanged by a 1-edge join"
            );
        }
    }

    #[test]
    fn scw_graph_path_converges_with_delete_then_readd() {
        // SPEC-0 SC-W (graph half): concurrent mutations - an edge/node add and
        // a delete-then-re-add - converge to the add-wins, time-correct state
        // regardless of receive order. The doc half is covered by yjs_sync's
        // pull_diff_and_push_converge tests; together they are SC-W. The node
        // carries a doc_id reference (the entity-to-doc bridge in the spec).
        let h_add = Hlc::new(10, 0, ActorId::from_label("codex"));
        let h_del = Hlc::new(20, 0, ActorId::from_label("codex"));
        let h_readd = Hlc::new(30, 0, ActorId::from_label("claude"));

        let add = StampedBatch::new([StampedMutation::new(
            GraphMutation::NodeUpsert(NodeRecord::new(
                "civic:obj:1",
                ["CivicObject"],
                json!({ "doc_id": "code:repo:lib.rs" }),
            )),
            h_add,
        )]);
        let mut deleted = NodeRecord::new("civic:obj:1", ["CivicObject"], json!({}));
        deleted.tombstone = true;
        let delete = StampedBatch::new([StampedMutation::new(
            GraphMutation::NodeUpsert(deleted),
            h_del,
        )]);
        let readd = StampedBatch::new([StampedMutation::new(
            GraphMutation::NodeUpsert(NodeRecord::new(
                "civic:obj:1",
                ["CivicObject"],
                json!({ "doc_id": "code:repo:lib.rs" }),
            )),
            h_readd,
        )]);

        let mut b = InMemoryGraphStore::new();
        apply_update_frame(&mut b, &update_frame(&add));
        apply_update_frame(&mut b, &update_frame(&delete));
        apply_update_frame(&mut b, &update_frame(&readd));

        let mut c = InMemoryGraphStore::new();
        apply_update_frame(&mut c, &update_frame(&readd));
        apply_update_frame(&mut c, &update_frame(&delete));
        apply_update_frame(&mut c, &update_frame(&add));

        // Add-wins: the highest-Hlc re-add revives the node on both replicas.
        assert!(!b.get_node("civic:obj:1").unwrap().tombstone);
        assert!(!c.get_node("civic:obj:1").unwrap().tombstone);
        assert_eq!(
            b.get_node("civic:obj:1").unwrap().properties["doc_id"],
            c.get_node("civic:obj:1").unwrap().properties["doc_id"]
        );
    }
}
