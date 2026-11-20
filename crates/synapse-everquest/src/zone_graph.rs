use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::map::{
    EverQuestMapColor, EverQuestMapCoord, EverQuestMapError, EverQuestMapFile, EverQuestMapLine,
    EverQuestMapPoint, EverQuestMapRecord, discover_map_files, parse_map_file,
};

#[derive(Debug, Error)]
pub enum EverQuestZoneGraphError {
    #[error(transparent)]
    Map(#[from] EverQuestMapError),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EverQuestZoneGraph {
    pub nodes: Vec<EverQuestZoneNode>,
    pub landmarks: Vec<EverQuestZoneLandmark>,
    pub edges: Vec<EverQuestZoneEdge>,
    pub segments: Vec<EverQuestZoneSegment>,
    pub unresolved_edge_count: usize,
    pub skipped_maps: Vec<EverQuestZoneSkippedMap>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EverQuestZoneNode {
    pub zone_short_name: String,
    pub display_name: Option<String>,
    pub source_path: PathBuf,
    pub len_bytes: u64,
    pub last_modified_unix_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EverQuestZoneLandmark {
    pub zone_short_name: String,
    pub label: String,
    pub normalized_label: String,
    pub location: EverQuestMapCoord,
    pub layer: i32,
    pub source_path: PathBuf,
    pub source_line_number: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EverQuestZoneEdge {
    pub source_zone_short_name: String,
    pub target_zone_short_name: Option<String>,
    pub target_display_name: Option<String>,
    pub target_hint: String,
    pub normalized_target_hint: String,
    pub label: String,
    pub location: EverQuestMapCoord,
    pub confidence: f32,
    pub resolution: EverQuestZoneEdgeResolution,
    pub source_path: PathBuf,
    pub source_line_number: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EverQuestZoneSegment {
    pub zone_short_name: String,
    pub start: EverQuestMapCoord,
    pub end: EverQuestMapCoord,
    pub color: EverQuestMapColor,
    pub source_path: PathBuf,
    pub source_line_number: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EverQuestZoneSkippedMap {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EverQuestZoneEdgeResolution {
    ExactZoneShortName,
    Alias,
    Unresolved,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EverQuestNearestLandmark {
    pub landmark: EverQuestZoneLandmark,
    pub distance: f64,
}

/// Builds a static zone graph from every parseable map file below an EQ install root.
///
/// # Errors
///
/// Returns [`EverQuestZoneGraphError`] if map discovery or any map parse fails.
pub fn build_zone_graph_from_root(
    root: &Path,
) -> Result<EverQuestZoneGraph, EverQuestZoneGraphError> {
    let mut maps = Vec::new();
    let mut skipped_maps = Vec::new();
    for source in discover_map_files(root)? {
        match parse_map_file(&source.path) {
            Ok(map) => maps.push(map),
            Err(error) => skipped_maps.push(EverQuestZoneSkippedMap {
                path: source.path,
                error: error.to_string(),
            }),
        }
    }
    let mut graph = build_zone_graph(&maps);
    graph.skipped_maps = skipped_maps;
    Ok(graph)
}

#[must_use]
pub fn build_zone_graph(maps: &[EverQuestMapFile]) -> EverQuestZoneGraph {
    let available_zones = maps
        .iter()
        .map(|map| map.source.zone_short_name.clone())
        .collect::<HashSet<_>>();
    let alias_index = build_alias_index(&available_zones);

    let mut nodes = Vec::with_capacity(maps.len());
    let mut landmarks = Vec::new();
    let mut edges = Vec::new();
    let mut segments = Vec::new();

    for map in maps {
        nodes.push(EverQuestZoneNode {
            zone_short_name: map.source.zone_short_name.clone(),
            display_name: display_name_for_zone(&map.source.zone_short_name),
            source_path: map.source.path.clone(),
            len_bytes: map.source.len_bytes,
            last_modified_unix_ms: map.source.last_modified_unix_ms,
        });

        for record in &map.records {
            match record {
                EverQuestMapRecord::Line(line) => {
                    segments.push(segment_from_line(&map.source.zone_short_name, line));
                }
                EverQuestMapRecord::Point(point) => {
                    landmarks.push(landmark_from_point(&map.source.zone_short_name, point));
                    if let Some(target_hint) = transition_target_hint(&point.label) {
                        edges.push(edge_from_point(
                            &map.source.zone_short_name,
                            point,
                            target_hint,
                            &available_zones,
                            &alias_index,
                        ));
                    }
                }
            }
        }
    }

    nodes.sort_by(|left, right| left.zone_short_name.cmp(&right.zone_short_name));
    landmarks.sort_by(|left, right| {
        left.zone_short_name
            .cmp(&right.zone_short_name)
            .then(left.source_line_number.cmp(&right.source_line_number))
    });
    edges.sort_by(|left, right| {
        left.source_zone_short_name
            .cmp(&right.source_zone_short_name)
            .then(left.source_line_number.cmp(&right.source_line_number))
    });
    segments.sort_by(|left, right| {
        left.zone_short_name
            .cmp(&right.zone_short_name)
            .then(left.source_line_number.cmp(&right.source_line_number))
    });
    let unresolved_edge_count = edges
        .iter()
        .filter(|edge| edge.resolution == EverQuestZoneEdgeResolution::Unresolved)
        .count();

    EverQuestZoneGraph {
        nodes,
        landmarks,
        edges,
        segments,
        unresolved_edge_count,
        skipped_maps: Vec::new(),
    }
}

impl EverQuestZoneGraph {
    #[must_use]
    pub fn node(&self, zone_short_name: &str) -> Option<&EverQuestZoneNode> {
        self.nodes
            .iter()
            .find(|node| node.zone_short_name.eq_ignore_ascii_case(zone_short_name))
    }

    #[must_use]
    pub fn exits_for_zone(&self, zone_short_name: &str) -> Vec<EverQuestZoneEdge> {
        self.edges
            .iter()
            .filter(|edge| {
                edge.source_zone_short_name
                    .eq_ignore_ascii_case(zone_short_name)
            })
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn landmarks_for_zone(&self, zone_short_name: &str) -> Vec<EverQuestZoneLandmark> {
        self.landmarks
            .iter()
            .filter(|landmark| {
                landmark
                    .zone_short_name
                    .eq_ignore_ascii_case(zone_short_name)
            })
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn segments_for_zone(&self, zone_short_name: &str) -> Vec<EverQuestZoneSegment> {
        self.segments
            .iter()
            .filter(|segment| {
                segment
                    .zone_short_name
                    .eq_ignore_ascii_case(zone_short_name)
            })
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn nearest_landmarks(
        &self,
        zone_short_name: &str,
        location: &EverQuestMapCoord,
        limit: usize,
    ) -> Vec<EverQuestNearestLandmark> {
        let mut nearest = self
            .landmarks_for_zone(zone_short_name)
            .into_iter()
            .map(|landmark| EverQuestNearestLandmark {
                distance: distance(&landmark.location, location),
                landmark,
            })
            .collect::<Vec<_>>();
        nearest.sort_by(|left, right| left.distance.total_cmp(&right.distance));
        nearest.truncate(limit);
        nearest
    }
}

fn landmark_from_point(zone_short_name: &str, point: &EverQuestMapPoint) -> EverQuestZoneLandmark {
    EverQuestZoneLandmark {
        zone_short_name: zone_short_name.to_owned(),
        label: point.label.clone(),
        normalized_label: normalize_label(&point.label),
        location: point.location.clone(),
        layer: point.layer,
        source_path: point.source_path.clone(),
        source_line_number: point.source_line_number,
    }
}

fn segment_from_line(zone_short_name: &str, line: &EverQuestMapLine) -> EverQuestZoneSegment {
    EverQuestZoneSegment {
        zone_short_name: zone_short_name.to_owned(),
        start: line.start.clone(),
        end: line.end.clone(),
        color: line.color.clone(),
        source_path: line.source_path.clone(),
        source_line_number: line.source_line_number,
    }
}

fn edge_from_point(
    source_zone_short_name: &str,
    point: &EverQuestMapPoint,
    target_hint: String,
    available_zones: &HashSet<String>,
    alias_index: &HashMap<String, String>,
) -> EverQuestZoneEdge {
    let normalized_target_hint = normalize_label(&target_hint);
    let exact_target = available_zones
        .iter()
        .find(|zone| normalize_label(zone) == normalized_target_hint)
        .cloned();
    let resolved = exact_target
        .map(|target| {
            (
                Some(target),
                EverQuestZoneEdgeResolution::ExactZoneShortName,
                0.95,
            )
        })
        .or_else(|| {
            alias_index.get(&normalized_target_hint).map(|target| {
                (
                    Some(target.clone()),
                    EverQuestZoneEdgeResolution::Alias,
                    0.85,
                )
            })
        });
    let (target_zone_short_name, resolution, confidence) =
        resolved.unwrap_or((None, EverQuestZoneEdgeResolution::Unresolved, 0.25));
    let target_display_name = target_zone_short_name
        .as_deref()
        .and_then(display_name_for_zone);

    EverQuestZoneEdge {
        source_zone_short_name: source_zone_short_name.to_owned(),
        target_zone_short_name,
        target_display_name,
        target_hint,
        normalized_target_hint,
        label: point.label.clone(),
        location: point.location.clone(),
        confidence,
        resolution,
        source_path: point.source_path.clone(),
        source_line_number: point.source_line_number,
    }
}

fn transition_target_hint(label: &str) -> Option<String> {
    let trimmed = label.trim();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["to_", "to ", "to-"] {
        if lower.starts_with(prefix) {
            let hint = trimmed[prefix.len()..].trim_matches(['_', '-', ' ']);
            if !hint.is_empty() {
                return Some(hint.to_owned());
            }
        }
    }
    None
}

fn build_alias_index(available_zones: &HashSet<String>) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for zone in available_zones {
        aliases.insert(normalize_label(zone), zone.clone());
    }
    for (alias, target) in zone_aliases() {
        if available_zones.contains(*target) {
            aliases.insert(normalize_label(alias), (*target).to_owned());
        }
    }
    aliases
}

const fn zone_aliases() -> &'static [(&'static str, &'static str)] {
    &[
        ("nektulos_forest", "nektulos"),
        ("nektulos", "nektulos"),
        ("neriak", "neriaka"),
        ("neriak_foreign_quarter", "neriaka"),
        ("neriak_commons", "neriakb"),
        ("neriak_third_gate", "neriakc"),
        ("east_commonlands", "ecommons"),
        ("west_commonlands", "commons"),
        ("commonlands", "commonlands"),
        ("lavastorm_mountains", "lavastorm"),
    ]
}

fn display_name_for_zone(zone_short_name: &str) -> Option<String> {
    let display = match zone_short_name.to_ascii_lowercase().as_str() {
        "nektulos" => "Nektulos Forest",
        "neriaka" => "Neriak - Foreign Quarter",
        "neriakb" => "Neriak - Commons",
        "neriakc" => "Neriak - Third Gate",
        "commonlands" => "Commonlands",
        "commons" => "West Commonlands",
        "ecommons" => "East Commonlands",
        "lavastorm" => "Lavastorm Mountains",
        _ => return None,
    };
    Some(display.to_owned())
}

fn normalize_label(label: &str) -> String {
    label
        .chars()
        .flat_map(char::to_lowercase)
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

fn distance(left: &EverQuestMapCoord, right: &EverQuestMapCoord) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    let dz = left.z - right.z;
    (dx.mul_add(dx, dy.mul_add(dy, dz * dz))).sqrt()
}
