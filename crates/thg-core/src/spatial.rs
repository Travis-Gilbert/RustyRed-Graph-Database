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

/// One spatial designation: a (lat, lon) property pair on a label that
/// should be H3-indexed at a given resolution.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpatialDesignation {
    pub label: String,
    pub lat_property: String,
    pub lon_property: String,
    pub resolution: u8,
}

#[derive(Debug, Default)]
pub struct SpatialIndex {
    pub designation: Option<SpatialDesignation>,
    pub cells: HashMap<CellIndex, Vec<String>>,
    pub node_to_cell: HashMap<String, (CellIndex, f64, f64)>, // node_id -> (cell, lat, lon)
}

impl SpatialIndex {
    pub fn for_designation(d: SpatialDesignation) -> Self {
        Self {
            designation: Some(d),
            cells: HashMap::new(),
            node_to_cell: HashMap::new(),
        }
    }

    pub fn upsert(&mut self, node_id: &str, lat: f64, lon: f64) -> Result<CellIndex, SpatialError> {
        let res = self.designation.as_ref().map(|d| d.resolution).unwrap_or(8);
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
        let res = self.designation.as_ref().map(|d| d.resolution).unwrap_or(8);
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
}

impl SpatialError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidResolution(_) => "invalid_resolution",
            Self::InvalidCoordinate(_) => "invalid_coordinate",
        }
    }
    pub fn message(&self) -> String {
        match self {
            Self::InvalidResolution(r) => format!("H3 resolution {r} is outside 0..=15"),
            Self::InvalidCoordinate(s) => format!("invalid coordinate: {s}"),
        }
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
}
