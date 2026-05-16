//! §P8-A pa8.2: S2-cell-backed spatial index. Behind the `s2` feature flag.
//! Selected at runtime via `RUSTY_RED_SPATIAL_BACKEND=s2` through the
//! `crate::spatial::make_spatial_backend` factory.
//!
//! H3 remains the default; S2 is the perf alternative the original SPEC named
//! (more accurate for polygon containment queries). Both implement
//! `SpatialBackend` so the rest of the system reads them through a uniform
//! interface.
//!
//! Implementation notes:
//! - Cells are computed at an S2 level derived from `designation.resolution`
//!   (which is sized to H3 0..15; we map `h3_resolution * 2` capped at 30).
//! - Radius search uses haversine post-filtering for accuracy. Cell-covering
//!   acceleration is a future optimization; for the bounded datasets the SPEC
//!   targets (`DEFAULT_LIMIT=100`, `MAX_LIMIT=1000`) a linear scan over indexed
//!   nodes is well within budget.

use std::collections::HashMap;

use s2::cellid::CellID;
use s2::latlng::LatLng as S2LatLng;

use crate::spatial::{SpatialBackend, SpatialDesignation, SpatialError};

/// S2 supports levels 0..=30. We size to H3's 0..=15 range.
const MAX_S2_LEVEL: u64 = 30;

fn level_from_h3_resolution(res: u8) -> u64 {
    (u64::from(res) * 2).min(MAX_S2_LEVEL)
}

fn lat_lng_degrees(lat: f64, lon: f64) -> S2LatLng {
    S2LatLng::from_degrees(lat, lon)
}

#[derive(Debug)]
pub struct S2SpatialBackend {
    designation: SpatialDesignation,
    cells: HashMap<u64, Vec<String>>,
    node_to_cell: HashMap<String, (u64, f64, f64)>,
    level: u64,
}

impl S2SpatialBackend {
    pub fn new(designation: SpatialDesignation) -> Self {
        let level = level_from_h3_resolution(designation.resolution);
        Self {
            designation,
            cells: HashMap::new(),
            node_to_cell: HashMap::new(),
            level,
        }
    }

    fn cell_for(&self, lat: f64, lon: f64) -> u64 {
        let cell = CellID::from(lat_lng_degrees(lat, lon)).parent(self.level);
        cell.0
    }
}

impl SpatialBackend for S2SpatialBackend {
    fn designation(&self) -> &SpatialDesignation {
        &self.designation
    }

    fn upsert(&mut self, node_id: &str, lat: f64, lon: f64) -> Result<(), SpatialError> {
        if !lat.is_finite() || !lon.is_finite() {
            return Err(SpatialError::InvalidCoordinate(format!(
                "non-finite coordinate ({lat}, {lon})"
            )));
        }
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err(SpatialError::InvalidCoordinate(format!(
                "out-of-range coordinate ({lat}, {lon})"
            )));
        }
        let cell = self.cell_for(lat, lon);
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
        Ok(())
    }

    fn remove(&mut self, node_id: &str) {
        if let Some((cell, _, _)) = self.node_to_cell.remove(node_id) {
            if let Some(vec) = self.cells.get_mut(&cell) {
                vec.retain(|id| id != node_id);
            }
        }
    }

    fn radius_search(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
    ) -> Result<Vec<String>, SpatialError> {
        if !lat.is_finite() || !lon.is_finite() {
            return Err(SpatialError::InvalidCoordinate(format!(
                "non-finite query coordinate ({lat}, {lon})"
            )));
        }
        let mut out: Vec<String> = self
            .node_to_cell
            .iter()
            .filter_map(|(id, (_, n_lat, n_lon))| {
                if haversine_km(lat, lon, *n_lat, *n_lon) <= radius_km {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn bbox_search(&self, min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Vec<String> {
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

    fn node_count(&self) -> usize {
        self.node_to_cell.len()
    }
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r_km = 6371.0_f64;
    let to_rad = std::f64::consts::PI / 180.0;
    let dlat = (lat2 - lat1) * to_rad;
    let dlon = (lon2 - lon1) * to_rad;
    let a = (dlat / 2.0).sin().powi(2)
        + (lat1 * to_rad).cos() * (lat2 * to_rad).cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r_km * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn designation() -> SpatialDesignation {
        SpatialDesignation {
            label: "Place".into(),
            lat_property: "lat".into(),
            lon_property: "lon".into(),
            resolution: 7,
        }
    }

    #[test]
    fn s2_backend_basic_upsert_and_radius_search() {
        let mut backend = S2SpatialBackend::new(designation());
        backend.upsert("sf", 37.7749, -122.4194).unwrap();
        backend.upsert("oak", 37.8044, -122.2712).unwrap();
        backend.upsert("nyc", 40.7128, -74.0060).unwrap();
        let near_sf = backend.radius_search(37.7749, -122.4194, 50.0).unwrap();
        assert!(near_sf.contains(&"sf".to_string()));
        assert!(near_sf.contains(&"oak".to_string()));
        assert!(!near_sf.contains(&"nyc".to_string()));
        assert_eq!(backend.node_count(), 3);
    }

    #[test]
    fn s2_backend_bbox_search_filters_to_box() {
        let mut backend = S2SpatialBackend::new(designation());
        backend.upsert("sf", 37.7749, -122.4194).unwrap();
        backend.upsert("nyc", 40.7128, -74.0060).unwrap();
        let bbox = backend.bbox_search(37.0, -123.0, 38.0, -122.0);
        assert_eq!(bbox, vec!["sf".to_string()]);
    }

    #[test]
    fn s2_backend_upsert_replaces_node_in_old_cell() {
        let mut backend = S2SpatialBackend::new(designation());
        backend.upsert("node", 37.7749, -122.4194).unwrap();
        let (old_cell, _, _) = backend.node_to_cell["node"];
        backend.upsert("node", 37.8, -120.0).unwrap();
        let (new_cell, _, _) = backend.node_to_cell["node"];
        assert_ne!(old_cell, new_cell);
        // Old cell either drained or doesn't reference the node.
        assert!(!backend
            .cells
            .get(&old_cell)
            .map(|v| v.contains(&"node".to_string()))
            .unwrap_or(false));
    }

    #[test]
    fn s2_backend_rejects_non_finite_coordinates() {
        let mut backend = S2SpatialBackend::new(designation());
        let err = backend
            .upsert("bad", f64::NAN, 0.0)
            .expect_err("NaN should error");
        assert_eq!(err.code(), "invalid_coordinate");
    }

    #[test]
    fn s2_backend_rejects_out_of_range_coordinates() {
        let mut backend = S2SpatialBackend::new(designation());
        let err = backend
            .upsert("bad", 95.0, 0.0)
            .expect_err("lat 95 should error");
        assert_eq!(err.code(), "invalid_coordinate");
    }
}
