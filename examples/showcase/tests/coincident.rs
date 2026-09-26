//! No two elements of one part may put same-facing faces on one plane in two
//! materials.
//!
//! Parts are not unions: two elements of a part are two solids and two draw
//! batches. When their faces are coplanar and overlap, they fight for depth as
//! parts and the kernel has to give each coplanar triangle to one operand when
//! they are unioned, which puts slivers of one material in the other. That is
//! an authoring rule the library cannot enforce cheaply, so this test finds the
//! defect and the kits offset their trim.
//!
//! Faces on one plane facing opposite ways are two solids touching, which is
//! how a kit stacks, so they are not a clash. Faces in the same slot are not a
//! clash either: one material on one plane is invisible, merged or not.
use std::collections::{BTreeMap, BTreeSet};

use ashlar::{Building, GeometryMesher, Part, TriangleMesh};
use ashlar_manifold::ManifoldMesher;
use ashlar_showcase::{Scene, building};
use glam::DVec3;

/// The overlap area, in square metres, above which two faces are said to clash.
const OVERLAP: f64 = 1e-6;

/// The plane an axis-aligned face lies in: which axis, which way it faces, and
/// the coordinate, bucketed to 1e-6 m so planes that agree there share a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PlaneKey {
    axis: u8,
    positive: bool,
    coordinate: i64,
}

#[test]
fn no_part_shares_a_plane_between_two_materials() {
    let mut clashes = BTreeSet::new();
    for (scene, variant, _) in Scene::ALL {
        let building = building(variant).expect("the scene builds");
        let used = used_parts(&building);
        for part in building
            .recipe()
            .parts
            .iter()
            .filter(|part| used.contains(part.id.as_str()))
        {
            clashes.extend(part_clashes(scene, part));
        }
    }
    assert!(
        clashes.is_empty(),
        "coincident faces between two materials:\n{}",
        clashes.into_iter().collect::<Vec<_>>().join("\n")
    );
}

/// The ids of the parts a scene places at least once. A part no instance names
/// is not drawn and cannot clash.
fn used_parts(building: &Building) -> BTreeSet<&str> {
    building
        .recipe()
        .instances
        .iter()
        .map(|instance| instance.part.as_str())
        .collect()
}

/// Every clash inside one part, as report lines: one per pair of elements that
/// share a same-facing plane in two materials, with the area they overlap by.
fn part_clashes(scene: &str, part: &Part) -> Vec<String> {
    let mesher = ManifoldMesher::default();
    // Plane, then element: the triangles of one element in that plane.
    let mut planes: BTreeMap<PlaneKey, BTreeMap<usize, Vec<[[f64; 2]; 3]>>> = BTreeMap::new();
    let mut bottom = f64::INFINITY;
    let mut top = f64::NEG_INFINITY;
    for (index, element) in part.elements.iter().enumerate() {
        let mesh = mesher
            .mesh(&element.geometry)
            .unwrap_or_else(|error| panic!("{scene}/{}/{}: {error}", part.id, element.id));
        for point in &mesh.positions {
            bottom = bottom.min(point.y);
            top = top.max(point.y);
        }
        for (key, points) in body_faces(&mesh) {
            planes
                .entry(key)
                .or_default()
                .entry(index)
                .or_default()
                .push(points);
        }
    }
    let mut clashes = Vec::new();
    for (key, elements) in &planes {
        if key.axis == 1 && is_stacking_plane(key.coordinate, bottom, top) {
            continue;
        }
        for (left_index, left_faces) in elements {
            for (right_index, right_faces) in elements.range((*left_index + 1)..) {
                let left = &part.elements[*left_index];
                let right = &part.elements[*right_index];
                if left.material_slot == right.material_slot {
                    continue;
                }
                let area: f64 = left_faces
                    .iter()
                    .flat_map(|left| {
                        right_faces
                            .iter()
                            .map(move |right| overlap_area(*left, *right))
                    })
                    .sum();
                if area > OVERLAP {
                    clashes.push(format!(
                        "{scene}: {}: {} ({}) and {} ({}) on {}={}, {area:.6} m^2",
                        part.id,
                        left.id,
                        left.material_slot,
                        right.id,
                        right.material_slot,
                        axis_name(key.axis),
                        bucket_coordinate(key.coordinate),
                    ));
                }
            }
        }
    }
    clashes
}

/// The axis-aligned body triangles of a mesh, each with the plane it lies in
/// and its two in-plane coordinates. Cut faces are ignored: they resolve to a
/// different slot and are not what a body face wears.
fn body_faces(mesh: &TriangleMesh) -> Vec<(PlaneKey, [[f64; 2]; 3])> {
    let mut faces = Vec::new();
    for (face, corners) in mesh.corners().enumerate() {
        if mesh.is_cut(face) {
            continue;
        }
        let points = corners.map(|index| mesh.positions[index]);
        let Some(normal) = (points[1] - points[0])
            .cross(points[2] - points[0])
            .try_normalize()
        else {
            continue;
        };
        let magnitude = normal.abs();
        if magnitude.max_element() <= 0.999_999 {
            continue;
        }
        let axis = magnitude.max_position();
        let coordinate = f64::midpoint(
            f64::midpoint(points[0][axis], points[1][axis]),
            points[2][axis],
        );
        faces.push((
            PlaneKey {
                axis: u8::try_from(axis).expect("an axis is 0, 1 or 2"),
                positive: normal[axis] > 0.0,
                coordinate: bucket_coordinate_key(coordinate),
            },
            project(points, axis),
        ));
    }
    faces
}

/// The two in-plane coordinates of a face lying on `axis`.
fn project(points: [DVec3; 3], axis: usize) -> [[f64; 2]; 3] {
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    points.map(|point| [point[u], point[v]])
}

/// Whether a Y plane is the part's own top or bottom, where it stacks.
///
/// The storey above or below meets a part on those two planes, so a merged
/// building unions them away and a part drawn as parts has a neighbour to hide
/// them. Shortening an element to clear them would open a gap between storeys,
/// so the lint exempts them. Nothing else is exempt: a face in the middle of a
/// part is never covered from both sides and has to be fixed.
fn is_stacking_plane(coordinate: i64, bottom: f64, top: f64) -> bool {
    let coordinate = bucket_coordinate(coordinate);
    (coordinate - bottom).abs() <= 1e-6 || (coordinate - top).abs() <= 1e-6
}

fn axis_name(axis: u8) -> &'static str {
    match axis {
        0 => "x",
        1 => "y",
        _ => "z",
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "a building coordinate in metres is far inside i64 range"
)]
fn bucket_coordinate_key(coordinate: f64) -> i64 {
    (coordinate / 1e-6).round() as i64
}

#[allow(
    clippy::cast_precision_loss,
    reason = "a metre coordinate near 1e7 is exact in f64"
)]
fn bucket_coordinate(bucket: i64) -> f64 {
    bucket as f64 * 1e-6
}

/// The area two triangles share in the plane, by clipping one against the
/// other. Sutherland-Hodgman needs a convex clip polygon, which a triangle
/// always is once its winding is made counter-clockwise.
fn overlap_area(left: [[f64; 2]; 3], right: [[f64; 2]; 3]) -> f64 {
    let subject = counter_clockwise(left.to_vec());
    let clip = counter_clockwise(right.to_vec());
    signed_area(&clip_to_convex(&subject, &clip)).abs()
}

/// The signed area of a simple polygon, positive when its winding is
/// counter-clockwise.
fn signed_area(polygon: &[[f64; 2]]) -> f64 {
    let mut sum = 0.0;
    for (index, &point) in polygon.iter().enumerate() {
        let next = polygon[(index + 1) % polygon.len()];
        sum += point[0] * next[1] - next[0] * point[1];
    }
    sum / 2.0
}

/// The same polygon wound counter-clockwise, whatever it started as.
fn counter_clockwise(mut polygon: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if signed_area(&polygon) < 0.0 {
        polygon.reverse();
    }
    polygon
}

/// Clip a convex-clipped subject against each edge of a convex `clip` polygon.
fn clip_to_convex(subject: &[[f64; 2]], clip: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut output = subject.to_vec();
    for (index, &edge_start) in clip.iter().enumerate() {
        if output.is_empty() {
            break;
        }
        let edge_end = clip[(index + 1) % clip.len()];
        let input = std::mem::take(&mut output);
        let mut previous = *input.last().expect("the input is not empty");
        for &current in &input {
            let current_inside = side(edge_start, edge_end, current) >= 0.0;
            let previous_inside = side(edge_start, edge_end, previous) >= 0.0;
            if current_inside != previous_inside
                && let Some(crossing) = intersection(previous, current, edge_start, edge_end)
            {
                output.push(crossing);
            }
            if current_inside {
                output.push(current);
            }
            previous = current;
        }
    }
    output
}

/// Which side of the directed line `start` to `end` a point is on: positive to
/// the left, so positive is inside a counter-clockwise polygon.
fn side(start: [f64; 2], end: [f64; 2], point: [f64; 2]) -> f64 {
    (end[0] - start[0]) * (point[1] - start[1]) - (end[1] - start[1]) * (point[0] - start[0])
}

/// Where segment `from` to `to` crosses the line `start` to `end`, if the two
/// are not parallel.
fn intersection(from: [f64; 2], to: [f64; 2], start: [f64; 2], end: [f64; 2]) -> Option<[f64; 2]> {
    let segment = [to[0] - from[0], to[1] - from[1]];
    let edge = [end[0] - start[0], end[1] - start[1]];
    let denominator = segment[0] * edge[1] - segment[1] * edge[0];
    if denominator.abs() < 1e-12 {
        return None;
    }
    let offset = [start[0] - from[0], start[1] - from[1]];
    let t = (offset[0] * edge[1] - offset[1] * edge[0]) / denominator;
    Some([from[0] + t * segment[0], from[1] + t * segment[1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_area_is_measured_by_clipping() {
        // Two unit right triangles meeting along their diagonal share no area.
        let lower = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        let upper = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        assert!(overlap_area(lower, upper).abs() < 1e-12);

        // A triangle against itself is its own area, a half a square metre.
        assert!((overlap_area(lower, lower) - 0.5).abs() < 1e-12);

        // A right triangle with legs two, shifted half a leg along X: the
        // overlap is the unit right triangle at (1, 0), (2, 0), (1, 1), so a
        // quarter of the original.
        let triangle = [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]];
        let shifted = [[1.0, 0.0], [3.0, 0.0], [1.0, 2.0]];
        assert!((overlap_area(triangle, shifted) - 0.5).abs() < 1e-12);
    }
}
