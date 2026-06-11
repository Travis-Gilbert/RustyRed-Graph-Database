use std::collections::{BTreeSet, HashMap};

use geo::{BoundingRect, Contains, Intersects, Within};
use geo_types::{Geometry, Point};
use geozero::wkb::Wkb;
use geozero::wkt::Wkt;
use geozero::{CoordDimensions, ToGeo, ToWkb, ToWkt};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::plugin::{PluginCapability, PluginCapabilityKind, RustyRedPlugin};
use crate::spatial::SpatialDesignation;

#[cfg(feature = "s2")]
use s2::cellid::CellID;
#[cfg(feature = "s2")]
use s2::latlng::LatLng as S2LatLng;
#[cfg(feature = "s2")]
use s2::rect::Rect as S2Rect;
#[cfg(feature = "s2")]
use s2::region::RegionCoverer;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GeometryEncoding {
    Point,
    Wkb,
    Wkt,
    Subgraph,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GeometryDesignation {
    pub label: String,
    pub property: String,
    pub encoding: GeometryEncoding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat_property: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon_property: Option<String>,
    pub resolution: u8,
}

impl GeometryDesignation {
    pub fn point(
        label: impl Into<String>,
        lat_property: impl Into<String>,
        lon_property: impl Into<String>,
        resolution: u8,
    ) -> Self {
        let lat_property = lat_property.into();
        let lon_property = lon_property.into();
        Self {
            label: label.into(),
            property: format!("{lat_property},{lon_property}"),
            encoding: GeometryEncoding::Point,
            lat_property: Some(lat_property),
            lon_property: Some(lon_property),
            resolution,
        }
    }

    pub fn property(
        label: impl Into<String>,
        property: impl Into<String>,
        encoding: GeometryEncoding,
        resolution: u8,
    ) -> Self {
        Self {
            label: label.into(),
            property: property.into(),
            encoding,
            lat_property: None,
            lon_property: None,
            resolution,
        }
    }
}

impl From<SpatialDesignation> for GeometryDesignation {
    fn from(value: SpatialDesignation) -> Self {
        Self::point(
            value.label,
            value.lat_property,
            value.lon_property,
            value.resolution,
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GeometryEncoderDescriptor {
    pub name: String,
    pub encoding: GeometryEncoding,
}

pub trait GeometryEncoder: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str;
    fn encoding(&self) -> GeometryEncoding;
    fn encode(
        &self,
        geom: &Geometry<f64>,
        props: &mut Value,
        designation: &GeometryDesignation,
    ) -> Result<(), GeometryError>;
    fn decode(
        &self,
        props: &Value,
        designation: &GeometryDesignation,
    ) -> Result<Geometry<f64>, GeometryError>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GeometryError {
    MissingProperty(String),
    InvalidProperty(String),
    UnsupportedEncoding(String),
    UnsupportedGeometry(String),
    Codec(String),
    Cover(String),
}

impl GeometryError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingProperty(_) => "missing_geometry_property",
            Self::InvalidProperty(_) => "invalid_geometry_property",
            Self::UnsupportedEncoding(_) => "unsupported_geometry_encoding",
            Self::UnsupportedGeometry(_) => "unsupported_geometry",
            Self::Codec(_) => "geometry_codec_error",
            Self::Cover(_) => "geometry_cover_error",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::MissingProperty(message)
            | Self::InvalidProperty(message)
            | Self::UnsupportedEncoding(message)
            | Self::UnsupportedGeometry(message)
            | Self::Codec(message)
            | Self::Cover(message) => message.clone(),
        }
    }
}

impl std::fmt::Display for GeometryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for GeometryError {}

#[derive(Clone, Debug)]
pub struct PointEncoder;

impl GeometryEncoder for PointEncoder {
    fn name(&self) -> &'static str {
        "point"
    }

    fn encoding(&self) -> GeometryEncoding {
        GeometryEncoding::Point
    }

    fn encode(
        &self,
        geom: &Geometry<f64>,
        props: &mut Value,
        designation: &GeometryDesignation,
    ) -> Result<(), GeometryError> {
        let point = match geom {
            Geometry::Point(point) => point,
            other => {
                return Err(GeometryError::UnsupportedGeometry(format!(
                    "PointEncoder cannot encode {other:?}"
                )))
            }
        };
        let lat_property = designation_lat_property(designation)?;
        let lon_property = designation_lon_property(designation)?;
        let object = props_object_mut(props)?;
        object.insert(
            lat_property.to_string(),
            Number::from_f64(point.y())
                .map(Value::Number)
                .ok_or_else(|| GeometryError::InvalidProperty("lat is non-finite".to_string()))?,
        );
        object.insert(
            lon_property.to_string(),
            Number::from_f64(point.x())
                .map(Value::Number)
                .ok_or_else(|| GeometryError::InvalidProperty("lon is non-finite".to_string()))?,
        );
        Ok(())
    }

    fn decode(
        &self,
        props: &Value,
        designation: &GeometryDesignation,
    ) -> Result<Geometry<f64>, GeometryError> {
        let lat_property = designation_lat_property(designation)?;
        let lon_property = designation_lon_property(designation)?;
        let lat = props
            .get(lat_property)
            .and_then(Value::as_f64)
            .ok_or_else(|| GeometryError::MissingProperty(lat_property.to_string()))?;
        let lon = props
            .get(lon_property)
            .and_then(Value::as_f64)
            .ok_or_else(|| GeometryError::MissingProperty(lon_property.to_string()))?;
        Ok(Geometry::Point(Point::new(lon, lat)))
    }
}

#[derive(Clone, Debug)]
pub struct WkbEncoder;

impl GeometryEncoder for WkbEncoder {
    fn name(&self) -> &'static str {
        "wkb"
    }

    fn encoding(&self) -> GeometryEncoding {
        GeometryEncoding::Wkb
    }

    fn encode(
        &self,
        geom: &Geometry<f64>,
        props: &mut Value,
        designation: &GeometryDesignation,
    ) -> Result<(), GeometryError> {
        let bytes = geom
            .to_wkb(CoordDimensions::xy())
            .map_err(|err| GeometryError::Codec(err.to_string()))?;
        let object = props_object_mut(props)?;
        object.insert(
            designation.property.clone(),
            Value::Array(
                bytes
                    .into_iter()
                    .map(|byte| Value::Number(Number::from(byte)))
                    .collect(),
            ),
        );
        Ok(())
    }

    fn decode(
        &self,
        props: &Value,
        designation: &GeometryDesignation,
    ) -> Result<Geometry<f64>, GeometryError> {
        let bytes = json_bytes(
            props
                .get(&designation.property)
                .ok_or_else(|| GeometryError::MissingProperty(designation.property.clone()))?,
        )?;
        Wkb(bytes.as_slice())
            .to_geo()
            .map_err(|err| GeometryError::Codec(err.to_string()))
    }
}

#[derive(Clone, Debug)]
pub struct WktEncoder;

impl GeometryEncoder for WktEncoder {
    fn name(&self) -> &'static str {
        "wkt"
    }

    fn encoding(&self) -> GeometryEncoding {
        GeometryEncoding::Wkt
    }

    fn encode(
        &self,
        geom: &Geometry<f64>,
        props: &mut Value,
        designation: &GeometryDesignation,
    ) -> Result<(), GeometryError> {
        let wkt = geom
            .to_wkt()
            .map_err(|err| GeometryError::Codec(err.to_string()))?;
        let object = props_object_mut(props)?;
        object.insert(designation.property.clone(), Value::String(wkt));
        Ok(())
    }

    fn decode(
        &self,
        props: &Value,
        designation: &GeometryDesignation,
    ) -> Result<Geometry<f64>, GeometryError> {
        let wkt = props
            .get(&designation.property)
            .and_then(Value::as_str)
            .ok_or_else(|| GeometryError::MissingProperty(designation.property.clone()))?;
        Wkt(wkt)
            .to_geo()
            .map_err(|err| GeometryError::Codec(err.to_string()))
    }
}

pub fn encoder_for_encoding(
    encoding: &GeometryEncoding,
) -> Result<Box<dyn GeometryEncoder>, GeometryError> {
    match encoding {
        GeometryEncoding::Point => Ok(Box::new(PointEncoder)),
        GeometryEncoding::Wkb => Ok(Box::new(WkbEncoder)),
        GeometryEncoding::Wkt => Ok(Box::new(WktEncoder)),
        GeometryEncoding::Subgraph => Err(GeometryError::UnsupportedEncoding(
            "subgraph geometry encoding is declared for consumers and is not a core scalar encoder"
                .to_string(),
        )),
    }
}

pub fn geometry_encoder_descriptors() -> Vec<GeometryEncoderDescriptor> {
    vec![
        GeometryEncoderDescriptor {
            name: "point".to_string(),
            encoding: GeometryEncoding::Point,
        },
        GeometryEncoderDescriptor {
            name: "wkb".to_string(),
            encoding: GeometryEncoding::Wkb,
        },
        GeometryEncoderDescriptor {
            name: "wkt".to_string(),
            encoding: GeometryEncoding::Wkt,
        },
    ]
}

pub fn decode_geometry_value(
    encoding: GeometryEncoding,
    value: &Value,
) -> Result<Geometry<f64>, GeometryError> {
    let designation = GeometryDesignation::property("__query", "geometry", encoding.clone(), 0);
    let encoder = encoder_for_encoding(&encoding)?;
    let props = serde_json::json!({ "geometry": value.clone() });
    encoder.decode(&props, &designation)
}

#[derive(Debug)]
pub struct GeometryIndex {
    designation: GeometryDesignation,
    encoder: Box<dyn GeometryEncoder>,
    geometries: HashMap<String, Geometry<f64>>,
    #[cfg(feature = "s2")]
    cells: HashMap<u64, Vec<String>>,
    #[cfg(feature = "s2")]
    node_to_cells: HashMap<String, Vec<u64>>,
}

impl GeometryIndex {
    pub fn new(designation: GeometryDesignation) -> Result<Self, GeometryError> {
        let encoder = encoder_for_encoding(&designation.encoding)?;
        Ok(Self {
            designation,
            encoder,
            geometries: HashMap::new(),
            #[cfg(feature = "s2")]
            cells: HashMap::new(),
            #[cfg(feature = "s2")]
            node_to_cells: HashMap::new(),
        })
    }

    pub fn designation(&self) -> &GeometryDesignation {
        &self.designation
    }

    pub fn node_count(&self) -> usize {
        self.geometries.len()
    }

    pub fn decode_properties(&self, props: &Value) -> Result<Geometry<f64>, GeometryError> {
        self.encoder.decode(props, &self.designation)
    }

    pub fn encode_geometry(
        &self,
        geom: &Geometry<f64>,
        props: &mut Value,
    ) -> Result<(), GeometryError> {
        self.encoder.encode(geom, props, &self.designation)
    }

    pub fn upsert_from_properties(
        &mut self,
        node_id: &str,
        props: &Value,
    ) -> Result<(), GeometryError> {
        let geom = self.decode_properties(props)?;
        self.upsert_geometry(node_id, geom)
    }

    pub fn upsert_geometry(
        &mut self,
        node_id: &str,
        geom: Geometry<f64>,
    ) -> Result<(), GeometryError> {
        #[cfg(feature = "s2")]
        {
            self.remove(node_id);
            let cover = cover_geometry(&geom, self.designation.resolution)?;
            for cell in &cover {
                self.cells
                    .entry(*cell)
                    .or_default()
                    .push(node_id.to_string());
            }
            self.node_to_cells
                .insert(node_id.to_string(), cover.into_iter().collect());
            self.geometries.insert(node_id.to_string(), geom);
            return Ok(());
        }
        #[cfg(not(feature = "s2"))]
        {
            let _ = node_id;
            let _ = geom;
            Err(GeometryError::Cover(
                "geometry indexing requires building with --features s2".to_string(),
            ))
        }
    }

    pub fn remove(&mut self, node_id: &str) {
        #[cfg(feature = "s2")]
        {
            if let Some(old_cells) = self.node_to_cells.remove(node_id) {
                for cell in old_cells {
                    if let Some(nodes) = self.cells.get_mut(&cell) {
                        nodes.retain(|id| id != node_id);
                    }
                }
            }
        }
        self.geometries.remove(node_id);
    }

    pub fn contains_point(&self, lat: f64, lon: f64) -> Result<Vec<String>, GeometryError> {
        let point = Point::new(lon, lat);
        let candidates = self.candidates_for_point(lat, lon)?;
        Ok(self.refine_candidates(candidates, |geom| match geom {
            Geometry::Polygon(_) | Geometry::MultiPolygon(_) => geom.contains(&point),
            _ => false,
        }))
    }

    pub fn intersects(&self, query: &Geometry<f64>) -> Result<Vec<String>, GeometryError> {
        let candidates = self.candidates_for_geometry(query)?;
        Ok(self.refine_candidates(candidates, |geom| geom.intersects(query)))
    }

    pub fn intersects_value(
        &self,
        encoding: GeometryEncoding,
        value: &Value,
    ) -> Result<Vec<String>, GeometryError> {
        let query = decode_geometry_value(encoding, value)?;
        self.intersects(&query)
    }

    pub fn within(&self, query: &Geometry<f64>) -> Result<Vec<String>, GeometryError> {
        let candidates = self.candidates_for_geometry(query)?;
        Ok(self.refine_candidates(candidates, |geom| geom.is_within(query)))
    }

    pub fn within_value(
        &self,
        encoding: GeometryEncoding,
        value: &Value,
    ) -> Result<Vec<String>, GeometryError> {
        let query = decode_geometry_value(encoding, value)?;
        self.within(&query)
    }

    fn refine_candidates(
        &self,
        candidates: BTreeSet<String>,
        predicate: impl Fn(&Geometry<f64>) -> bool,
    ) -> Vec<String> {
        let mut out: Vec<String> = candidates
            .into_iter()
            .filter(|node_id| {
                self.geometries
                    .get(node_id)
                    .is_some_and(|geom| predicate(geom))
            })
            .collect();
        out.sort();
        out
    }

    fn candidates_for_point(&self, lat: f64, lon: f64) -> Result<BTreeSet<String>, GeometryError> {
        #[cfg(feature = "s2")]
        {
            let cell = cell_for_lat_lon(lat, lon, self.designation.resolution)?;
            return Ok(self.candidates_for_cell(cell));
        }
        #[cfg(not(feature = "s2"))]
        {
            let _ = (lat, lon);
            Err(GeometryError::Cover(
                "geometry candidates require building with --features s2".to_string(),
            ))
        }
    }

    fn candidates_for_geometry(
        &self,
        geom: &Geometry<f64>,
    ) -> Result<BTreeSet<String>, GeometryError> {
        #[cfg(feature = "s2")]
        {
            let mut out = BTreeSet::new();
            for cell in cover_geometry(geom, self.designation.resolution)? {
                out.extend(self.candidates_for_cell(cell));
            }
            return Ok(out);
        }
        #[cfg(not(feature = "s2"))]
        {
            let _ = geom;
            Err(GeometryError::Cover(
                "geometry candidates require building with --features s2".to_string(),
            ))
        }
    }

    #[cfg(feature = "s2")]
    fn candidates_for_cell(&self, cell: u64) -> BTreeSet<String> {
        let query_cell = CellID(cell);
        self.cells
            .iter()
            .filter(|(stored_cell, _)| CellID(**stored_cell).intersects(&query_cell))
            .flat_map(|(_, nodes)| nodes.iter().cloned())
            .collect()
    }

    #[cfg(feature = "s2")]
    pub fn cells_for_node(&self, node_id: &str) -> Vec<u64> {
        self.node_to_cells.get(node_id).cloned().unwrap_or_default()
    }

    #[cfg(feature = "s2")]
    pub fn cell_contains_node(&self, cell: u64, node_id: &str) -> bool {
        self.cells
            .get(&cell)
            .is_some_and(|nodes| nodes.iter().any(|id| id == node_id))
    }
}

#[derive(Clone, Debug)]
pub struct GeometryPlugin;

impl RustyRedPlugin for GeometryPlugin {
    fn name(&self) -> &'static str {
        "rustyred.geometry"
    }

    fn capabilities(&self) -> Vec<PluginCapability> {
        let mut capabilities = vec![
            PluginCapability {
                kind: PluginCapabilityKind::Designation,
                name: "geometry.designation".to_string(),
            },
            PluginCapability {
                kind: PluginCapabilityKind::Index,
                name: "geometry.s2_cover".to_string(),
            },
            PluginCapability {
                kind: PluginCapabilityKind::Hook,
                name: "geometry.node_upsert_index".to_string(),
            },
        ];
        capabilities.extend(
            geometry_encoder_descriptors()
                .into_iter()
                .map(|descriptor| PluginCapability {
                    kind: PluginCapabilityKind::Encoder,
                    name: format!("geometry.encoder.{}", descriptor.name),
                }),
        );
        capabilities
    }
}

fn props_object_mut(
    props: &mut Value,
) -> Result<&mut serde_json::Map<String, Value>, GeometryError> {
    if props.is_null() {
        *props = Value::Object(Default::default());
    }
    props
        .as_object_mut()
        .ok_or_else(|| GeometryError::InvalidProperty("properties must be a JSON object".into()))
}

fn designation_lat_property(designation: &GeometryDesignation) -> Result<&str, GeometryError> {
    designation
        .lat_property
        .as_deref()
        .ok_or_else(|| GeometryError::InvalidProperty("lat_property is required".to_string()))
}

fn designation_lon_property(designation: &GeometryDesignation) -> Result<&str, GeometryError> {
    designation
        .lon_property
        .as_deref()
        .ok_or_else(|| GeometryError::InvalidProperty("lon_property is required".to_string()))
}

fn json_bytes(value: &Value) -> Result<Vec<u8>, GeometryError> {
    let bytes = value
        .as_array()
        .ok_or_else(|| GeometryError::InvalidProperty("WKB property must be a byte array".into()))?
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|byte| u8::try_from(byte).ok())
                .ok_or_else(|| {
                    GeometryError::InvalidProperty(
                        "WKB byte array must contain integers in 0..=255".to_string(),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if bytes.is_empty() {
        return Err(GeometryError::InvalidProperty(
            "WKB property cannot be empty".to_string(),
        ));
    }
    Ok(bytes)
}

#[cfg(feature = "s2")]
const MAX_S2_LEVEL: u8 = 30;
#[cfg(feature = "s2")]
const MAX_S2_COVER_CELLS: usize = 1024;

#[cfg(feature = "s2")]
fn level_from_resolution(resolution: u8) -> u8 {
    resolution.saturating_mul(2).clamp(1, MAX_S2_LEVEL)
}

#[cfg(feature = "s2")]
fn cell_for_lat_lon(lat: f64, lon: f64, resolution: u8) -> Result<u64, GeometryError> {
    validate_coord(lat, lon)?;
    let level = level_from_resolution(resolution);
    let cell = CellID::from(S2LatLng::from_degrees(lat, lon)).parent(u64::from(level));
    Ok(cell.0)
}

#[cfg(feature = "s2")]
fn cover_geometry(geom: &Geometry<f64>, resolution: u8) -> Result<BTreeSet<u64>, GeometryError> {
    let level = level_from_resolution(resolution);
    match geom {
        Geometry::Point(point) => {
            let mut cells = BTreeSet::new();
            cells.insert(cell_for_lat_lon(point.y(), point.x(), resolution)?);
            Ok(cells)
        }
        _ => {
            let rect = geom.bounding_rect().ok_or_else(|| {
                GeometryError::Cover("cannot cover geometry without a bounding rectangle".into())
            })?;
            let min_lat = rect.min().y;
            let max_lat = rect.max().y;
            let min_lon = rect.min().x;
            let max_lon = rect.max().x;
            validate_coord(min_lat, min_lon)?;
            validate_coord(max_lat, max_lon)?;

            let coverer = RegionCoverer {
                min_level: 1,
                max_level: level,
                level_mod: 1,
                max_cells: MAX_S2_COVER_CELLS,
            };
            let s2_rect = S2Rect::from_degrees(min_lat, min_lon, max_lat, max_lon);
            Ok(coverer
                .covering(&s2_rect)
                .0
                .into_iter()
                .map(|cell| cell.0)
                .collect())
        }
    }
}

#[cfg(feature = "s2")]
fn validate_coord(lat: f64, lon: f64) -> Result<(), GeometryError> {
    if !lat.is_finite() || !lon.is_finite() {
        return Err(GeometryError::InvalidProperty(format!(
            "non-finite coordinate ({lat}, {lon})"
        )));
    }
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(GeometryError::InvalidProperty(format!(
            "out-of-range coordinate ({lat}, {lon})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo_types::{coord, LineString, Polygon};
    use serde_json::json;

    fn square() -> Geometry<f64> {
        Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                coord! { x: 0.0, y: 0.0 },
                coord! { x: 4.0, y: 0.0 },
                coord! { x: 4.0, y: 4.0 },
                coord! { x: 0.0, y: 4.0 },
                coord! { x: 0.0, y: 0.0 },
            ]),
            Vec::new(),
        ))
    }

    #[test]
    fn wkb_encoder_round_trips_polygon_and_linestring() {
        let encoder = WkbEncoder;
        let designation = GeometryDesignation::property("Parcel", "geom", GeometryEncoding::Wkb, 7);
        let geometries = vec![
            square(),
            Geometry::LineString(LineString::from(vec![
                coord! { x: -1.0, y: 1.0 },
                coord! { x: 5.0, y: 1.0 },
            ])),
        ];

        for geom in geometries {
            let mut props = json!({});
            encoder.encode(&geom, &mut props, &designation).unwrap();
            let decoded = encoder.decode(&props, &designation).unwrap();
            assert_eq!(decoded, geom);
        }
    }

    #[test]
    fn wkt_encoder_round_trips_polygon() {
        let encoder = WktEncoder;
        let designation = GeometryDesignation::property("Parcel", "geom", GeometryEncoding::Wkt, 7);
        let mut props = json!({});
        encoder.encode(&square(), &mut props, &designation).unwrap();

        let decoded = encoder.decode(&props, &designation).unwrap();
        assert_eq!(decoded, square());
    }

    #[test]
    fn point_encoder_matches_lat_lon_properties() {
        let encoder = PointEncoder;
        let designation = GeometryDesignation::point("Place", "lat", "lon", 7);
        let mut props = json!({});
        let point = Geometry::Point(Point::new(-83.6875, 43.0125));

        encoder.encode(&point, &mut props, &designation).unwrap();
        assert_eq!(props["lat"], 43.0125);
        assert_eq!(props["lon"], -83.6875);
        assert_eq!(encoder.decode(&props, &designation).unwrap(), point);
    }

    #[test]
    fn geometry_plugin_enumerates_encoders() {
        let plugin = GeometryPlugin;
        let capabilities = plugin.capabilities();

        assert!(capabilities
            .iter()
            .any(|capability| capability.name == "geometry.encoder.point"));
        assert!(capabilities
            .iter()
            .any(|capability| capability.name == "geometry.encoder.wkb"));
        assert!(capabilities
            .iter()
            .any(|capability| capability.name == "geometry.encoder.wkt"));
    }

    #[cfg(feature = "s2")]
    #[test]
    fn geometry_index_stores_and_retrieves_polygon_from_properties() {
        let designation = GeometryDesignation::property("Parcel", "geom", GeometryEncoding::Wkb, 7);
        let mut index = GeometryIndex::new(designation).unwrap();
        let polygon = square();
        let mut props = json!({});
        index.encode_geometry(&polygon, &mut props).unwrap();

        index.upsert_from_properties("parcel:1", &props).unwrap();

        assert_eq!(index.node_count(), 1);
        assert_eq!(index.decode_properties(&props).unwrap(), polygon);
        assert!(!index.cells_for_node("parcel:1").is_empty());
    }

    #[cfg(feature = "s2")]
    #[test]
    fn point_geometry_indexes_to_one_cell() {
        let designation = GeometryDesignation::point("Place", "lat", "lon", 7);
        let mut index = GeometryIndex::new(designation).unwrap();
        index
            .upsert_geometry("point:1", Geometry::Point(Point::new(-83.7, 43.0)))
            .unwrap();

        assert_eq!(index.cells_for_node("point:1").len(), 1);
        assert_eq!(
            index.contains_point(43.0, -83.7).unwrap(),
            Vec::<String>::new()
        );
    }

    #[cfg(feature = "s2")]
    #[test]
    fn geometry_index_moves_between_covers_without_stale_cells() {
        let designation = GeometryDesignation::property("Parcel", "geom", GeometryEncoding::Wkb, 7);
        let mut index = GeometryIndex::new(designation).unwrap();
        let first = square();
        index.upsert_geometry("parcel:1", first).unwrap();
        let old_cells = index.cells_for_node("parcel:1");
        let shifted = Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                coord! { x: 20.0, y: 20.0 },
                coord! { x: 24.0, y: 20.0 },
                coord! { x: 24.0, y: 24.0 },
                coord! { x: 20.0, y: 24.0 },
                coord! { x: 20.0, y: 20.0 },
            ]),
            Vec::new(),
        ));

        index.upsert_geometry("parcel:1", shifted).unwrap();

        assert_ne!(old_cells, index.cells_for_node("parcel:1"));
        assert!(old_cells
            .into_iter()
            .all(|cell| !index.cell_contains_node(cell, "parcel:1")));
    }

    #[cfg(feature = "s2")]
    #[test]
    fn topology_predicates_refine_broad_phase_candidates() {
        let designation = GeometryDesignation::property("Parcel", "geom", GeometryEncoding::Wkb, 7);
        let mut index = GeometryIndex::new(designation).unwrap();
        index.upsert_geometry("parcel:1", square()).unwrap();
        index
            .upsert_geometry(
                "parcel:2",
                Geometry::Polygon(Polygon::new(
                    LineString::from(vec![
                        coord! { x: 10.0, y: 10.0 },
                        coord! { x: 14.0, y: 10.0 },
                        coord! { x: 14.0, y: 14.0 },
                        coord! { x: 10.0, y: 14.0 },
                        coord! { x: 10.0, y: 10.0 },
                    ]),
                    Vec::new(),
                )),
            )
            .unwrap();

        assert_eq!(
            index.contains_point(2.0, 2.0).unwrap(),
            vec!["parcel:1".to_string()]
        );
        assert!(index.contains_point(8.0, 8.0).unwrap().is_empty());

        let crossing = Geometry::LineString(LineString::from(vec![
            coord! { x: -1.0, y: 1.0 },
            coord! { x: 5.0, y: 1.0 },
        ]));
        assert_eq!(index.intersects(&crossing).unwrap(), vec!["parcel:1"]);

        let envelope = Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                coord! { x: -1.0, y: -1.0 },
                coord! { x: 5.0, y: -1.0 },
                coord! { x: 5.0, y: 5.0 },
                coord! { x: -1.0, y: 5.0 },
                coord! { x: -1.0, y: -1.0 },
            ]),
            Vec::new(),
        ));
        assert_eq!(index.within(&envelope).unwrap(), vec!["parcel:1"]);
    }
}
