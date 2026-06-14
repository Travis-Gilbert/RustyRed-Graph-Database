//! Civic projection job: a one-way, idempotent mirror of `civic:*` yjs docs
//! into the tenant graph store.
//!
//! The auto-organizer plan (Open Flint Atlas,
//! `docs/plans/porchfest-planner/auto-organizer-projection-plan.md`) treats
//! the BlockSuite/Yjs civic-object store as the source of truth and RustyRed
//! as a read-only projection surface: rows become `civic_object` graph nodes
//! so engine jobs (entity resolution, gap demons, spatial queries) can run
//! over them. RustyRed never writes back into the CRDT doc.
//!
//! The decoder mirrors the proven Node spike
//! (`scripts/spike-civic-doc-projection.mjs`) at the raw CRDT level, with no
//! BlockSuite involvement:
//!
//! - root map `civic:column-ids`: fieldKey -> columnId (the contract key map
//!   maintained by civic-workspace.ts)
//! - root map `blocks`: blockId -> Y.Map with `sys:flavour`, `sys:children`,
//!   `prop:columns`, `prop:cells`, `prop:text`
//! - select and multi-select cells hold option ids resolved through the
//!   column's `data.options` list
//!
//! Projection contract:
//! - node id `civic-row:<docId>:<rowId>`, label `civic_object`, properties
//!   are the decoded fields plus `title`, `rowId`, `sourceDoc`, and a
//!   `projectedAt` marker. The doc id is part of the node id because block
//!   ids survive doc snapshots verbatim: a civic:porchfest-2027 doc seeded
//!   from the 2026 snapshot must not fight the 2026 projection for nodes.
//! - a `location` field that parses as `{"lng":number,"lat":number}` also
//!   sets numeric `lat`/`lon` properties so a point geometry designation
//!   can index the node
//! - rows that disappear from the doc are tombstoned (the store's mutation
//!   surface is upsert-only; an upsert with `tombstone: true` removes the
//!   node from store reads, fulltext, and geometry indexes; the H3 spatial
//!   hook gained a symmetric remove branch for the same guarantee)
//! - only allowlisted tenants project (RUSTY_RED_CIVIC_PROJECTION_TENANTS,
//!   comma-separated). The yjs route is deliberately unauthenticated for
//!   browser sync, so without the allowlist anyone could mint civic_object
//!   nodes in any tenant by naming a civic: doc; the allowlist confines the
//!   projection write path to tenants the operator owns.
//!
//! Deliberate divergences from the Node spike, both stricter: multi-select
//! values keep falsy resolved options (the spike's filter(Boolean) dropped
//! them) and select ids must be strings (the spike coerced numbers through
//! JS object-key lookup). Structural-guard parity with the spike is exact:
//! a doc missing `civic:column-ids`, the database block, `prop:columns`,
//! `prop:cells`, or `sys:children` errors instead of decoding empty, so a
//! half-synced doc can never field-wipe or mass-tombstone an existing
//! projection.
//!
//! Failures here log and never break the sync path.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustyred_core::{GraphMutation, GraphMutationBatch, NodeQuery, NodeRecord};
use serde_json::{json, Map as JsonMap, Value};
use yrs::types::ToJson;
use yrs::{Doc, Map, MapRef, Out, ReadTxn, Transact};

use crate::router::commit_batch_with_indexes;
use crate::state::AppState;
use crate::yjs_sync::YjsRoom;

/// Only docs in the civic namespace are projected.
pub(crate) const CIVIC_DOC_PREFIX: &str = "civic:";
/// Projected node ids are `civic-row:<docId>:<rowId>`.
const CIVIC_NODE_ID_PREFIX: &str = "civic-row:";
/// Hard cap on rows decoded from one doc: the sync route is open by design,
/// so an unbounded crafted doc must not mint unbounded graph nodes.
const CIVIC_MAX_ROWS: usize = 10_000;
/// Comma-separated tenant allowlist for projection. Empty or unset disables
/// projection entirely (sync itself is unaffected).
const CIVIC_PROJECTION_TENANTS_ENV: &str = "RUSTY_RED_CIVIC_PROJECTION_TENANTS";
/// Label shared by every projected row; also the geometry designation label.
const CIVIC_LABEL: &str = "civic_object";
/// Point designations map resolution to S2 levels (resolution * 2, clamped
/// to 30); 16 lands on the finest level for point-in-cell lookups.
#[cfg(feature = "geometry")]
const CIVIC_GEOMETRY_RESOLUTION: u8 = 16;
/// One projection per edit burst, not per keystroke.
const PROJECTION_DEBOUNCE_MS: u64 = 400;

/// A decoded civic row: plain data, no CRDT types.
#[derive(Clone, Debug)]
pub struct CivicRow {
    pub row_id: String,
    pub title: String,
    pub fields: JsonMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct CivicProjectionReport {
    pub rows: usize,
    pub removed: usize,
}

/// Decode the civic database rows out of a BlockSuite-shaped yrs doc.
///
/// Errors when the doc is not (yet) civic-shaped: a doc mid-creation has no
/// `civic:column-ids` map or no `affine:database` block. Erroring (instead of
/// decoding zero fields) protects an existing projection from being
/// overwritten by a half-synced doc.
pub fn decode_civic_rows(doc: &Doc) -> Result<Vec<CivicRow>, String> {
    let txn = doc.transact();

    let column_ids = txn
        .get_map("civic:column-ids")
        .ok_or("doc has no civic:column-ids map")?;
    let mut field_key_by_column_id: BTreeMap<String, String> = BTreeMap::new();
    for (field_key, value) in column_ids.iter(&txn) {
        if let Some(column_id) = out_to_value(&value, &txn).as_str() {
            field_key_by_column_id.insert(column_id.to_string(), field_key.to_string());
        }
    }

    let blocks = txn.get_map("blocks").ok_or("doc has no blocks map")?;
    let mut database: Option<MapRef> = None;
    for (_, out) in blocks.iter(&txn) {
        if let Out::YMap(block) = out {
            let flavour = block
                .get(&txn, "sys:flavour")
                .map(|out| out_to_value(&out, &txn));
            if flavour.as_ref().and_then(Value::as_str) == Some("affine:database") {
                database = Some(block);
                break;
            }
        }
    }
    let database = database.ok_or("no affine:database block in the doc")?;

    // columnId -> (option id -> option value), for select and multi-select.
    // Missing prop:columns is a structural error like the spike's throw:
    // decoding without option maps would project raw option ids over
    // previously resolved values.
    let columns = database
        .get(&txn, "prop:columns")
        .map(|out| out_to_value(&out, &txn))
        .ok_or("database block has no prop:columns")?;
    let mut option_value_by_column_id: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    if let Some(columns) = columns.as_array() {
        for column in columns {
            let Some(column_id) = column.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(options) = column
                .get("data")
                .and_then(|data| data.get("options"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            let mut option_values = BTreeMap::new();
            for option in options {
                if let (Some(option_id), Some(option_value)) = (
                    option.get("id").and_then(Value::as_str),
                    option.get("value"),
                ) {
                    option_values.insert(option_id.to_string(), option_value.clone());
                }
            }
            option_value_by_column_id.insert(column_id.to_string(), option_values);
        }
    }

    // Missing prop:cells would decode every row empty and the
    // wholesale-replace upsert would wipe all projected fields: error, like
    // the spike.
    let cells = database
        .get(&txn, "prop:cells")
        .map(|out| out_to_value(&out, &txn))
        .ok_or("database block has no prop:cells")?;
    let empty_cells = JsonMap::new();

    // Row order comes from the database block's children, like the spike.
    // Missing sys:children would decode zero rows and tombstone every live
    // node for the doc: error, like the spike.
    let row_ids: Vec<String> = database
        .get(&txn, "sys:children")
        .map(|out| out_to_value(&out, &txn))
        .and_then(|value| value.as_array().cloned())
        .map(|items| {
            items
                .into_iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .ok_or("database block has no sys:children")?;
    if row_ids.len() > CIVIC_MAX_ROWS {
        return Err(format!(
            "doc has {} rows, over the {} projection cap",
            row_ids.len(),
            CIVIC_MAX_ROWS
        ));
    }

    let mut rows = Vec::with_capacity(row_ids.len());
    for row_id in row_ids {
        let title = match blocks.get(&txn, &row_id) {
            Some(Out::YMap(row_block)) => row_block
                .get(&txn, "prop:text")
                .map(|out| out_to_value(&out, &txn))
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            _ => String::new(),
        };
        let row_cells = cells
            .get(&row_id)
            .and_then(Value::as_object)
            .unwrap_or(&empty_cells);
        let mut fields = JsonMap::new();
        for (column_id, cell) in row_cells {
            let Some(field_key) = field_key_by_column_id.get(column_id) else {
                continue;
            };
            let Some(value) = cell.get("value") else {
                continue;
            };
            if value.is_null() {
                continue;
            }
            let resolved = match option_value_by_column_id.get(column_id) {
                Some(option_values) => match value {
                    // Multi-select: resolve ids, drop ids with no option.
                    Value::Array(ids) => Value::Array(
                        ids.iter()
                            .filter_map(|id| {
                                id.as_str().and_then(|id| option_values.get(id)).cloned()
                            })
                            .collect(),
                    ),
                    // Select: unknown ids pass through unresolved, like the spike.
                    other => other
                        .as_str()
                        .and_then(|id| option_values.get(id))
                        .cloned()
                        .unwrap_or_else(|| other.clone()),
                },
                None => value.clone(),
            };
            fields.insert(field_key.clone(), resolved);
        }
        rows.push(CivicRow {
            row_id,
            title,
            fields,
        });
    }
    Ok(rows)
}

/// Upsert decoded rows into the tenant graph and tombstone projections whose
/// rows left the doc. Idempotent: the same doc state yields the same set of
/// live `civic_object` nodes.
pub fn apply_civic_rows(
    state: &AppState,
    tenant_id: &str,
    doc_id: &str,
    rows: &[CivicRow],
) -> Result<CivicProjectionReport, String> {
    // The geometry designation is registered lazily so a tenant that never
    // syncs a civic doc carries no index. Failures must not block the
    // projection: nodes still get lat/lon properties, so a later successful
    // registration bulk-indexes them.
    #[cfg(feature = "geometry")]
    if let Err(error) = state.ensure_geometry_point_designation(
        tenant_id,
        CIVIC_LABEL,
        "lat",
        "lon",
        CIVIC_GEOMETRY_RESOLUTION,
    ) {
        tracing::warn!(
            tenant_id,
            error = %error.message,
            "civic projection: geometry designation unavailable"
        );
    }

    let mut store = state
        .tenant_graph_store(tenant_id)
        .map_err(|error| error.message)?;

    // Diff base: live projected rows for THIS doc. Tombstoned nodes are
    // filtered by the store, so removed rows are tombstoned exactly once.
    let existing = store
        .query_nodes(NodeQuery::label(CIVIC_LABEL))
        .map_err(|error| error.message)?;
    let mut stale: std::collections::BTreeSet<String> = existing
        .iter()
        .filter(|node| {
            node.id.starts_with(CIVIC_NODE_ID_PREFIX)
                && node.properties.get("sourceDoc").and_then(Value::as_str) == Some(doc_id)
        })
        .map(|node| node.id.clone())
        .collect();

    let projected_at = now_ms();
    let mut mutations = Vec::with_capacity(rows.len() + stale.len());
    for row in rows {
        // Doc-scoped ids: block ids survive doc snapshots verbatim, so a new
        // year's doc seeded from last year's snapshot must not steal nodes.
        let node_id = format!("{CIVIC_NODE_ID_PREFIX}{doc_id}:{}", row.row_id);
        stale.remove(&node_id);
        let mut properties = row.fields.clone();
        // Markers land after the fields so they win any key collision.
        properties.insert("title".to_string(), Value::from(row.title.clone()));
        properties.insert("rowId".to_string(), Value::from(row.row_id.clone()));
        properties.insert("sourceDoc".to_string(), Value::from(doc_id));
        properties.insert("projectedAt".to_string(), Value::from(projected_at));
        if let Some((lat, lon)) = parse_location(row.fields.get("location")) {
            properties.insert("lat".to_string(), Value::from(lat));
            properties.insert("lon".to_string(), Value::from(lon));
        }
        mutations.push(GraphMutation::NodeUpsert(NodeRecord::new(
            node_id,
            [CIVIC_LABEL],
            Value::Object(properties),
        )));
    }

    let removed = stale.len();
    for node_id in stale {
        // Upsert-only store: tombstone: true is the deletion mechanism. The
        // tombstone keeps no lat/lon, so the post-commit index hooks evict
        // the node from the geometry index as well.
        let mut node = NodeRecord::new(
            node_id,
            [CIVIC_LABEL],
            json!({
                "sourceDoc": doc_id,
                "projectedAt": projected_at,
            }),
        );
        node.tombstone = true;
        mutations.push(GraphMutation::NodeUpsert(node));
    }

    if !mutations.is_empty() {
        commit_batch_with_indexes(
            state,
            tenant_id,
            &mut store,
            GraphMutationBatch::new(mutations),
        )
        .map_err(|error| error.message)?;
    }

    Ok(CivicProjectionReport {
        rows: rows.len(),
        removed,
    })
}

/// Tenants allowed to project, from RUSTY_RED_CIVIC_PROJECTION_TENANTS.
/// The sync route is open by design (browser collaboration carries no
/// token), so the projection write path is confined to operator-owned
/// tenants; unset or empty means projection is off everywhere.
fn projection_allowed(tenant_id: &str) -> bool {
    static ALLOWLIST: OnceLock<Vec<String>> = OnceLock::new();
    let allowlist = ALLOWLIST.get_or_init(|| {
        std::env::var(CIVIC_PROJECTION_TENANTS_ENV)
            .unwrap_or_default()
            .split(',')
            .map(|tenant| tenant.trim().to_string())
            .filter(|tenant| !tenant.is_empty())
            .collect()
    });
    allowlist.iter().any(|tenant| tenant == tenant_id)
}

/// Schedule a debounced projection for a room after an applied push. Only
/// `civic:*` docs in allowlisted tenants are projected; everything else
/// returns immediately.
pub(crate) fn schedule_civic_projection(state: &AppState, tenant_id: &str, room: &Arc<YjsRoom>) {
    if !room.doc_id.starts_with(CIVIC_DOC_PREFIX) || !projection_allowed(tenant_id) {
        return;
    }
    let generation = bump_projection_generation(&room.key);
    let state = state.clone();
    let tenant_id = tenant_id.to_string();
    let room = Arc::clone(room);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(PROJECTION_DEBOUNCE_MS)).await;
        if latest_projection_generation(&room.key) != generation {
            // A newer push restarted the debounce window; that task projects.
            return;
        }
        // Decode under the room lock, write to the store after releasing it,
        // so a slow store never stalls the sync path.
        let decoded = {
            let doc = room.doc.lock().await;
            decode_civic_rows(&doc)
        };
        // Re-check after decode: a push that landed while we decoded owns
        // the projection now. This narrows (not closes) the overlap window;
        // an apply already in flight converges on the next edit.
        if latest_projection_generation(&room.key) != generation {
            return;
        }
        match decoded {
            Ok(rows) => match apply_civic_rows(&state, &tenant_id, &room.doc_id, &rows) {
                Ok(report) => tracing::debug!(
                    room = %room.key,
                    rows = report.rows,
                    removed = report.removed,
                    "civic projection applied"
                ),
                Err(error) => tracing::warn!(
                    room = %room.key,
                    error = %error,
                    "civic projection failed; sync unaffected"
                ),
            },
            // A civic doc mid-creation is legitimately not database-shaped
            // yet; debug, not warn, to keep first-sync logs quiet.
            Err(error) => tracing::debug!(
                room = %room.key,
                error = %error,
                "civic doc not projectable yet"
            ),
        }
    });
}

/// Per-room debounce generations. A push bumps the generation; the spawned
/// task only projects if its generation is still the latest after the delay.
static PROJECTION_GENERATIONS: OnceLock<Mutex<BTreeMap<String, u64>>> = OnceLock::new();

fn projection_generations() -> &'static Mutex<BTreeMap<String, u64>> {
    PROJECTION_GENERATIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn bump_projection_generation(room_key: &str) -> u64 {
    let mut generations = projection_generations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let slot = generations.entry(room_key.to_string()).or_insert(0);
    *slot += 1;
    *slot
}

fn latest_projection_generation(room_key: &str) -> u64 {
    let generations = projection_generations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    generations.get(room_key).copied().unwrap_or(0)
}

/// Parse the civic `location` field: a JSON string (the schema stores it
/// stringified) or an inline object, with numeric `lng` and `lat`.
fn parse_location(value: Option<&Value>) -> Option<(f64, f64)> {
    let value = value?;
    let parsed;
    let object = match value {
        Value::String(raw) => {
            parsed = serde_json::from_str::<Value>(raw).ok()?;
            parsed.as_object()?
        }
        Value::Object(map) => map,
        _ => return None,
    };
    let lat = object.get("lat").and_then(Value::as_f64)?;
    let lon = object.get("lng").and_then(Value::as_f64)?;
    if !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    Some((lat, lon))
}

/// Convert any yrs value (shared type or primitive) into plain JSON; YText
/// becomes its string content, nested YMap/YArray recurse.
fn out_to_value<T: ReadTxn>(out: &Out, txn: &T) -> Value {
    serde_json::to_value(out.to_json(txn)).unwrap_or(Value::Null)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, StorageMode};
    use rustyred_core::RedCoreDurability;
    use yrs::{Any, Array, ArrayPrelim, MapPrelim, TextPrelim};

    fn memory_state() -> AppState {
        AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Memory,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::None,
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
            federate: false,
            federate_hub_url: None,
            federate_token: None,
            federate_peer_id: None,
            federate_private_key: None,
            federate_provenance: false,
            federate_snapshot_text_bytes: rustyred_search::DEFAULT_WEB_COMMONS_SNAPSHOT_TEXT_BYTES,
            mcp_enabled: false,
            mcp_read_only: true,
            mcp_allow_admin: false,
            mcp_default_tenant: "default".to_string(),
        })
    }

    const DOC_ID: &str = "civic:porchfest-2026";

    /// Build a doc mirroring the BlockSuite database layout the spike decodes:
    /// `civic:column-ids` contract map, an `affine:database` block with
    /// columns (select options live under `data.options`), per-row cells, and
    /// row blocks whose `prop:text` is a Y.Text title.
    fn build_civic_doc() -> Doc {
        let doc = Doc::new();
        let column_ids = doc.get_or_insert_map("civic:column-ids");
        let blocks = doc.get_or_insert_map("blocks");
        let mut txn = doc.transact_mut();

        column_ids.insert(&mut txn, "category", "col-category");
        column_ids.insert(&mut txn, "email", "col-email");
        column_ids.insert(&mut txn, "location", "col-location");
        column_ids.insert(&mut txn, "tags", "col-tags");

        let database = blocks.insert(&mut txn, "block-db", MapPrelim::default());
        database.insert(&mut txn, "sys:flavour", "affine:database");
        database.insert(
            &mut txn,
            "sys:children",
            ArrayPrelim::from(["row-1", "row-2"]),
        );
        database.insert(
            &mut txn,
            "prop:columns",
            Any::from_json(
                r#"[
                    {"id": "col-category", "type": "select", "name": "Category",
                     "data": {"options": [
                        {"id": "opt-band", "value": "band", "color": "var(--c1)"},
                        {"id": "opt-porch", "value": "porch", "color": "var(--c2)"}]}},
                    {"id": "col-email", "type": "rich-text", "name": "Email", "data": {}},
                    {"id": "col-location", "type": "rich-text", "name": "Location", "data": {}},
                    {"id": "col-tags", "type": "multi-select", "name": "Tags",
                     "data": {"options": [
                        {"id": "opt-acoustic", "value": "acoustic"},
                        {"id": "opt-loud", "value": "loud"}]}}
                ]"#,
            )
            .unwrap(),
        );
        // Cells are a Y.Map keyed by row id (BlockSuite nests deep Y types);
        // the cell objects themselves arrive as plain Any payloads here, and
        // the decoder's to_json path covers both shapes.
        let cells = database.insert(&mut txn, "prop:cells", MapPrelim::default());
        cells.insert(
            &mut txn,
            "row-1",
            Any::from_json(
                r#"{
                    "col-category": {"value": "opt-band"},
                    "col-location": {"value": "{\"lng\":-83.6875,\"lat\":43.0125}"},
                    "col-tags": {"value": ["opt-acoustic", "opt-loud", "opt-unknown"]},
                    "col-unmapped": {"value": "dropped"}
                }"#,
            )
            .unwrap(),
        );
        cells.insert(
            &mut txn,
            "row-2",
            Any::from_json(
                r#"{
                    "col-email": {"value": "porch@flint.org"},
                    "col-category": {"value": null}
                }"#,
            )
            .unwrap(),
        );

        let row_1 = blocks.insert(&mut txn, "row-1", MapPrelim::default());
        row_1.insert(&mut txn, "sys:flavour", "affine:database-row");
        row_1.insert(&mut txn, "prop:text", TextPrelim::new("Whaley House Porch"));
        let row_2 = blocks.insert(&mut txn, "row-2", MapPrelim::default());
        row_2.insert(&mut txn, "sys:flavour", "affine:database-row");
        row_2.insert(
            &mut txn,
            "prop:text",
            TextPrelim::new("Carriage Town Stage"),
        );
        drop(txn);
        doc
    }

    #[test]
    fn decoder_mirrors_the_spike_contract() {
        let doc = build_civic_doc();
        let rows = decode_civic_rows(&doc).unwrap();
        assert_eq!(rows.len(), 2);

        // Row order follows the database block's children.
        let row_1 = &rows[0];
        assert_eq!(row_1.row_id, "row-1");
        assert_eq!(row_1.title, "Whaley House Porch");
        assert_eq!(row_1.fields["category"], json!("band"));
        // Multi-select resolves option ids and drops unknown ids.
        assert_eq!(row_1.fields["tags"], json!(["acoustic", "loud"]));
        // Location passes through as the stored JSON string.
        assert_eq!(
            row_1.fields["location"],
            json!("{\"lng\":-83.6875,\"lat\":43.0125}")
        );
        // Cells under columns missing from civic:column-ids are skipped.
        assert_eq!(row_1.fields.len(), 3);

        let row_2 = &rows[1];
        assert_eq!(row_2.row_id, "row-2");
        assert_eq!(row_2.title, "Carriage Town Stage");
        assert_eq!(row_2.fields["email"], json!("porch@flint.org"));
        // Null cells are omitted entirely.
        assert!(!row_2.fields.contains_key("category"));
        assert_eq!(row_2.fields.len(), 1);
    }

    /// Decode then apply, the same two steps the debounced task runs.
    fn project(state: &AppState, doc: &Doc) -> CivicProjectionReport {
        let rows = decode_civic_rows(doc).unwrap();
        apply_civic_rows(state, "flint", DOC_ID, &rows).unwrap()
    }

    #[test]
    fn projection_is_idempotent_and_sets_coordinates() {
        let state = memory_state();
        let doc = build_civic_doc();

        let first = project(&state, &doc);
        let second = project(&state, &doc);
        assert_eq!(first.rows, 2);
        assert_eq!(first.removed, 0);
        assert_eq!(second.rows, 2);
        assert_eq!(second.removed, 0);

        let store = state.tenant_graph_store("flint").unwrap();
        let nodes = store.query_nodes(NodeQuery::label(CIVIC_LABEL)).unwrap();
        assert_eq!(nodes.len(), 2);

        let placed = store
            .get_node("civic-row:civic:porchfest-2026:row-1")
            .unwrap()
            .unwrap();
        assert!(placed.labels.iter().any(|label| label == CIVIC_LABEL));
        assert_eq!(placed.properties["title"], json!("Whaley House Porch"));
        assert_eq!(placed.properties["rowId"], json!("row-1"));
        assert_eq!(placed.properties["sourceDoc"], json!(DOC_ID));
        assert!(placed.properties["projectedAt"].is_u64());
        assert_eq!(placed.properties["category"], json!("band"));
        // The parsed location becomes numeric lat/lon for the point
        // geometry designation.
        assert_eq!(placed.properties["lat"], json!(43.0125));
        assert_eq!(placed.properties["lon"], json!(-83.6875));

        let unplaced = store
            .get_node("civic-row:civic:porchfest-2026:row-2")
            .unwrap()
            .unwrap();
        assert!(unplaced.properties.get("lat").is_none());
        assert!(unplaced.properties.get("lon").is_none());
    }

    #[test]
    fn removed_rows_lose_their_nodes_on_reprojection() {
        let state = memory_state();
        let doc = build_civic_doc();
        project(&state, &doc);

        // Delete row-2 the way a workspace edit would: drop it from the
        // database children, its cells, and the blocks map.
        {
            let blocks = doc.get_or_insert_map("blocks");
            let mut txn = doc.transact_mut();
            let Some(Out::YMap(database)) = blocks.get(&txn, "block-db") else {
                panic!("database block missing");
            };
            let Some(Out::YArray(children)) = database.get(&txn, "sys:children") else {
                panic!("children array missing");
            };
            children.remove(&mut txn, 1);
            let Some(Out::YMap(cells)) = database.get(&txn, "prop:cells") else {
                panic!("cells map missing");
            };
            cells.remove(&mut txn, "row-2");
            blocks.remove(&mut txn, "row-2");
        }

        let report = project(&state, &doc);
        assert_eq!(report.rows, 1);
        assert_eq!(report.removed, 1);

        let store = state.tenant_graph_store("flint").unwrap();
        let nodes = store.query_nodes(NodeQuery::label(CIVIC_LABEL)).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "civic-row:civic:porchfest-2026:row-1");
        // Tombstoned nodes vanish from point reads as well.
        assert!(store
            .get_node("civic-row:civic:porchfest-2026:row-2")
            .unwrap()
            .is_none());

        // Re-projecting the shrunk doc tombstones nothing new.
        let again = project(&state, &doc);
        assert_eq!(again.removed, 0);
    }

    #[cfg(feature = "geometry")]
    #[test]
    fn geometry_point_designation_registers_once_per_tenant() {
        let state = memory_state();
        assert!(state
            .ensure_geometry_point_designation(
                "flint",
                CIVIC_LABEL,
                "lat",
                "lon",
                CIVIC_GEOMETRY_RESOLUTION
            )
            .unwrap());
        assert!(!state
            .ensure_geometry_point_designation(
                "flint",
                CIVIC_LABEL,
                "lat",
                "lon",
                CIVIC_GEOMETRY_RESOLUTION
            )
            .unwrap());
    }
}
