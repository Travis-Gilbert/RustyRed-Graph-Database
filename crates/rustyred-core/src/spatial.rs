//! Phase 8: H3-based spatial index for nodes that carry geographic
//! coordinates.
//!
//! Per (label, lat_property, lon_property, resolution) we maintain a
//! `HashMap<CellIndex, Vec<node_id>>`. Radius queries find the H3 disk
//! around a point and union the cells; bbox queries fall back to a
//! linear scan over the indexed nodes (good enough for the bounded sizes
//! we expect; an R-tree is a later optimization).

use std::collections::HashMap;

use h3o::{CellIndex, LatLng, Resolution};
use serde::{Deserialize, Serialize};

/// §P8-A pa8.3: env var that selects the spatial backend at construction time.
pub const RUSTY_RED_SPATIAL_BACKEND_ENV: &str = "RUSTY_RED_SPATIAL_BACKEND";

// Canonical backend-name strings; the dispatcher accepts a few aliases per kind.
pub(crate) const SPATIAL_BACKEND_H3: &str = "h3";
pub(crate) const SPATIAL_BACKEND_S2: &str = "s2";

/// One spatial designation: a (lat, lon) property pair on a label that
/// should be H3-indexed at a given resolution.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpatialDesignation {
    pub label: String,
    pub lat_property: String,
    pub lon_property: String,
    pub resolution: u8,
}

/// §P8-A pa8.1: trait abstraction over the spatial storage layer. H3 is the
/// default impl; an S2-cell impl lives behind the `s2` feature flag.
pub trait SpatialBackend: Send + Sync + std::fmt::Debug {
    fn designation(&self) -> &SpatialDesignation;
    fn upsert(&mut self, node_id: &str, lat: f64, lon: f64) -> Result<(), SpatialError>;
    fn remove(&mut self, node_id: &str);
    fn radius_search(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
    ) -> Result<Vec<String>, SpatialError>;
    fn bbox_search(&self, min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Vec<String>;
    fn node_count(&self) -> usize;
}

/// §cc.3 (spatial half): designation is now a required, owned field. The
/// per-tenant map keys already carry (label, lat_property, lon_property),
/// so wrapping the designation in `Option` was redundant.
#[derive(Debug)]
pub struct SpatialIndex {
    pub designation: SpatialDesignation,
    // Storage is `pub(crate)` so other `rustyred-core` modules (and tests in this
    // file) can inspect it, while external crates must go through the trait.
    pub(crate) cells: HashMap<CellIndex, Vec<String>>,
    pub(crate) node_to_cell: HashMap<String, (CellIndex, f64, f64)>, // node_id -> (cell, lat, lon)
}

impl SpatialIndex {
    pub fn for_designation(d: SpatialDesignation) -> Self {
        Self {
            designation: d,
            cells: HashMap::new(),
            node_to_cell: HashMap::new(),
        }
    }

    pub fn upsert(&mut self, node_id: &str, lat: f64, lon: f64) -> Result<CellIndex, SpatialError> {
        let res = self.designation.resolution;
        let resolution =
            Resolution::try_from(res).map_err(|_| SpatialError::InvalidResolution(res))?;
        let cell = LatLng::new(lat, lon)
            .map_err(|err| SpatialError::InvalidCoordinate(format!("{err}")))?
            .to_cell(resolution);
        // remove from prior cell if needed
        if let Some((old_cell, _, _)) = self.node_to_cell.get(node_id).copied() {
            if let Some(vec) = self.cells.get_mut(&old_cell) {
                vec.retain(|id| id != node_id);
            }
        }
        self.cells
            .entry(cell)
            .or_default()
            .push(node_id.to_string());
        self.node_to_cell
            .insert(node_id.to_string(), (cell, lat, lon));
        Ok(cell)
    }

    pub fn remove(&mut self, node_id: &str) {
        if let Some((cell, _, _)) = self.node_to_cell.remove(node_id) {
            if let Some(vec) = self.cells.get_mut(&cell) {
                vec.retain(|id| id != node_id);
            }
        }
    }

    /// Radius search in kilometers. Returns node_ids whose stored coordinate
    /// is within `radius_km` of (lat, lon).
    pub fn radius_search(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
    ) -> Result<Vec<String>, SpatialError> {
        let res = self.designation.resolution;
        let resolution =
            Resolution::try_from(res).map_err(|_| SpatialError::InvalidResolution(res))?;
        let center_ll = LatLng::new(lat, lon)
            .map_err(|err| SpatialError::InvalidCoordinate(format!("{err}")))?;
        let center_cell = center_ll.to_cell(resolution);

        // Approximate the disk in cell-counts. Use the cell's edge length.
        let edge_km = resolution.edge_length_km();
        let k = ((radius_km / edge_km).ceil() as i32).max(1) as u32;
        let candidate_cells = center_cell.grid_disk::<Vec<_>>(k);

        let mut out: Vec<String> = Vec::new();
        for cell in candidate_cells {
            if let Some(nodes) = self.cells.get(&cell) {
                for node_id in nodes {
                    if let Some((_, n_lat, n_lon)) = self.node_to_cell.get(node_id) {
                        if haversine_km(lat, lon, *n_lat, *n_lon) <= radius_km {
                            out.push(node_id.clone());
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Axis-aligned bounding-box search. Performs a linear scan over indexed
    /// nodes (since H3 cells don't align with lat/lon rectangles).
    pub fn bbox_search(
        &self,
        min_lat: f64,
        min_lon: f64,
        max_lat: f64,
        max_lon: f64,
    ) -> Vec<String> {
        let mut out: Vec<String> = self
            .node_to_cell
            .iter()
            .filter_map(|(id, (_, lat, lon))| {
                if *lat >= min_lat && *lat <= max_lat && *lon >= min_lon && *lon <= max_lon {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        out.sort();
        out
    }
}

#[derive(Debug, Clone)]
pub enum SpatialError {
    InvalidResolution(u8),
    InvalidCoordinate(String),
    /// §P8-A pa8.3: env-switch selected a backend that isn't compiled in,
    /// or used an unknown name.
    UnknownBackend(String),
}

impl SpatialError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidResolution(_) => "invalid_resolution",
            Self::InvalidCoordinate(_) => "invalid_coordinate",
            Self::UnknownBackend(_) => "unknown_spatial_backend",
        }
    }
    pub fn message(&self) -> String {
        match self {
            Self::InvalidResolution(r) => format!("H3 resolution {r} is outside 0..=15"),
            Self::InvalidCoordinate(s) => format!("invalid coordinate: {s}"),
            Self::UnknownBackend(s) => format!("unknown spatial backend: {s}"),
        }
    }
}

/// Haversine distance between two (lat, lon) points, in kilometers. Shared by
/// both spatial backends (H3 here, S2 in `spatial_s2.rs`).
pub(crate) fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r_km = 6371.0_f64;
    let to_rad = std::f64::consts::PI / 180.0;
    let dlat = (lat2 - lat1) * to_rad;
    let dlon = (lon2 - lon1) * to_rad;
    let a = (dlat / 2.0).sin().powi(2)
        + (lat1 * to_rad).cos() * (lat2 * to_rad).cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r_km * a.sqrt().asin()
}

// §P8-A pa8.1: SpatialBackend trait impl for the default (H3) index. Signature
// matches the existing inherent `upsert`/`remove`/etc., minus the cell-return
// detail (the trait returns `()`).
impl SpatialBackend for SpatialIndex {
    fn designation(&self) -> &SpatialDesignation {
        &self.designation
    }

    fn upsert(&mut self, node_id: &str, lat: f64, lon: f64) -> Result<(), SpatialError> {
        SpatialIndex::upsert(self, node_id, lat, lon).map(|_| ())
    }

    fn remove(&mut self, node_id: &str) {
        SpatialIndex::remove(self, node_id);
    }

    fn radius_search(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
    ) -> Result<Vec<String>, SpatialError> {
        SpatialIndex::radius_search(self, lat, lon, radius_km)
    }

    fn bbox_search(&self, min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Vec<String> {
        SpatialIndex::bbox_search(self, min_lat, min_lon, max_lat, max_lon)
    }

    fn node_count(&self) -> usize {
        self.node_to_cell.len()
    }
}

/// §P8-A pa8.3: env-switch factory. Reads `RUSTY_RED_SPATIAL_BACKEND` and
/// forwards to the pure dispatcher below.
pub fn make_spatial_backend(
    designation: SpatialDesignation,
) -> Result<Box<dyn SpatialBackend>, SpatialError> {
    let raw = std::env::var(RUSTY_RED_SPATIAL_BACKEND_ENV).unwrap_or_default();
    make_spatial_backend_from_value(designation, &raw)
}

/// Pure dispatcher: takes the env-var value as an explicit parameter so unit
/// tests can run in parallel without mutating global state. Default ("" /
/// "h3" / "hand_rolled" / "hand-rolled") returns the H3 impl. `"s2"` returns
/// the S2 impl when compiled with `--features s2`; otherwise an explicit
/// error so the caller knows to rebuild.
pub fn make_spatial_backend_from_value(
    designation: SpatialDesignation,
    raw: &str,
) -> Result<Box<dyn SpatialBackend>, SpatialError> {
    let registry = crate::plugin::builtin_plugin_registry();
    if let Some(backend) = registry.spatial_backend(raw) {
        return (backend.constructor)(designation);
    }
    Err(SpatialError::UnknownBackend(format!(
        "unknown {RUSTY_RED_SPATIAL_BACKEND_ENV} value: {}",
        raw.trim().to_ascii_lowercase()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_search_includes_close_points_only() {
        let mut idx = SpatialIndex::for_designation(SpatialDesignation {
            label: "Place".into(),
            lat_property: "lat".into(),
            lon_property: "lon".into(),
            resolution: 7,
        });

        // San Francisco
        idx.upsert("sf", 37.7749, -122.4194).unwrap();
        // Oakland (close)
        idx.upsert("oak", 37.8044, -122.2712).unwrap();
        // New York (far)
        idx.upsert("nyc", 40.7128, -74.0060).unwrap();

        let near_sf = idx.radius_search(37.7749, -122.4194, 50.0).unwrap();
        assert!(near_sf.contains(&"sf".to_string()));
        assert!(near_sf.contains(&"oak".to_string()));
        assert!(!near_sf.contains(&"nyc".to_string()));
    }

    #[test]
    fn bbox_search_returns_only_nodes_inside_box() {
        let mut idx = SpatialIndex::for_designation(SpatialDesignation {
            label: "Place".into(),
            lat_property: "lat".into(),
            lon_property: "lon".into(),
            resolution: 7,
        });

        idx.upsert("sf", 37.7749, -122.4194).unwrap();
        idx.upsert("nyc", 40.7128, -74.0060).unwrap();

        let bbox = idx.bbox_search(37.0, -123.0, 38.0, -122.0);
        assert_eq!(bbox, vec!["sf".to_string()]);
    }

    #[test]
    fn upsert_moves_node_between_cells() {
        let mut idx = SpatialIndex::for_designation(SpatialDesignation {
            label: "Place".into(),
            lat_property: "lat".into(),
            lon_property: "lon".into(),
            resolution: 9,
        });
        idx.upsert("node", 37.7749, -122.4194).unwrap();
        let old_cell = idx.node_to_cell["node"].0;
        idx.upsert("node", 37.8, -122.0).unwrap();
        let new_cell = idx.node_to_cell["node"].0;
        assert_ne!(old_cell, new_cell);
        // old cell should no longer reference the node
        assert!(!idx
            .cells
            .get(&old_cell)
            .map(|v| v.contains(&"node".to_string()))
            .unwrap_or(false));
    }

    // §P8-A pa8.1 + cc.3 (spatial): backend trait + designation-required tests.

    #[test]
    fn h3_spatial_index_implements_spatial_backend_trait() {
        let designation = SpatialDesignation {
            label: "Place".into(),
            lat_property: "lat".into(),
            lon_property: "lon".into(),
            resolution: 7,
        };
        let mut backend: Box<dyn SpatialBackend> =
            Box::new(SpatialIndex::for_designation(designation));
        SpatialBackend::upsert(backend.as_mut(), "sf", 37.7749, -122.4194).unwrap();
        SpatialBackend::upsert(backend.as_mut(), "oak", 37.8044, -122.2712).unwrap();
        let near = backend.radius_search(37.7749, -122.4194, 50.0).unwrap();
        assert!(near.contains(&"sf".to_string()));
        assert!(near.contains(&"oak".to_string()));
        assert_eq!(backend.node_count(), 2);
    }

    #[test]
    fn spatial_designation_is_owned_value_not_optional() {
        let idx = SpatialIndex::for_designation(SpatialDesignation {
            label: "Place".into(),
            lat_property: "lat".into(),
            lon_property: "lon".into(),
            resolution: 7,
        });
        assert_eq!(idx.designation.label, "Place");
        assert_eq!(idx.designation.resolution, 7);
    }

    fn fixture_designation() -> SpatialDesignation {
        SpatialDesignation {
            label: "Place".into(),
            lat_property: "lat".into(),
            lon_property: "lon".into(),
            resolution: 7,
        }
    }

    #[test]
    fn make_spatial_backend_defaults_to_h3() {
        let backend = make_spatial_backend_from_value(fixture_designation(), "").unwrap();
        assert_eq!(backend.designation().label, "Place");
    }

    #[test]
    fn make_spatial_backend_rejects_unknown_backend() {
        let err = make_spatial_backend_from_value(fixture_designation(), "rtree")
            .expect_err("unknown backend should error");
        assert_eq!(err.code(), "unknown_spatial_backend");
    }

    #[cfg(not(feature = "s2"))]
    #[test]
    fn make_spatial_backend_errors_when_s2_requested_without_feature() {
        let err = make_spatial_backend_from_value(fixture_designation(), "s2")
            .expect_err("s2 backend without feature should error");
        assert_eq!(err.code(), "unknown_spatial_backend");
        assert!(err.message().to_ascii_lowercase().contains("s2"));
    }
}
