//! Corner shading normals, computed from an indexed mesh without a solid kernel.
use glam::DVec3;
use std::collections::HashMap;

/// How far below the sharp angle an edge must be to smooth, in degrees.
///
/// An edge at the sharp angle is a crease. This is a thousand times the noise
/// measured on the storey-merged tower, where 1e-9 degrees decided such an edge
/// corner by corner, and far below anything an author means by an angle: a 45
/// degree chamfer under the default 45 degree threshold keeps its faces flat
/// rather than being decided by rounding.
pub const SHARP_ANGLE_TIE: f64 = 1e-6;

/// One `(triangle, corner)` meeting a vertex, with the edge and angle at that
/// vertex needed to decide which neighbours it smooths with.
#[derive(Clone)]
struct Incident {
    triangle: usize,
    corner: usize,
    others: [u32; 2],
    angle: f64,
}

/// Compute the unit shading normal of every triangle corner of an indexed mesh.
///
/// The result has one `[DVec3; 3]` per triangle, in triangle order: the normal
/// of each of the triangle's three corners, in the same corner order as
/// `indices`. Two triangles are neighbours across an edge exactly when they
/// name the same two vertex indices, so the input must share vertices rather
/// than repeat positions; positions are never compared and vertices are never
/// welded.
///
/// An edge is smooth when the angle between the face normals of the two
/// triangles across it is strictly less than `sharp_angle - SHARP_ANGLE_TIE`
/// degrees, and a crease otherwise. An edge exactly at the sharp angle is a
/// crease, so a 45 degree chamfer under a 45 degree threshold keeps its faces
/// flat instead of being decided by floating-point rounding. A corner's normal
/// is the sum of the face normals of the triangles it smooths with, each
/// weighted by that triangle's interior angle at the corner. The weighting
/// makes the result independent of how a flat face happens to be cut into
/// triangles.
///
/// Returns `None` when `indices` is not a multiple of three long, when an index
/// is out of range, when a position or `sharp_angle` is not finite, or when a
/// triangle is degenerate (its cross product is zero or not finite). Empty
/// `indices` returns `Some(vec![])`.
///
/// ```
/// use ashlar::{shade_normals, glam::DVec3};
///
/// let positions = [
///     DVec3::new(0.0, 0.0, 0.0),
///     DVec3::new(1.0, 0.0, 0.0),
///     DVec3::new(0.0, 1.0, 0.0),
/// ];
/// let normals = shade_normals(&positions, &[0, 1, 2], 30.0).expect("valid triangle");
/// assert_eq!(normals, vec![[DVec3::Z; 3]]);
/// ```
pub fn shade_normals(
    positions: &[DVec3],
    indices: &[u32],
    sharp_angle: f64,
) -> Option<Vec<[DVec3; 3]>> {
    if !sharp_angle.is_finite() || !indices.len().is_multiple_of(3) {
        return None;
    }
    if positions.iter().any(|p| !p.is_finite()) {
        return None;
    }

    let triangle_count = indices.len() / 3;
    let mut face_normals = Vec::with_capacity(triangle_count);
    let mut corner_angles = Vec::with_capacity(triangle_count);
    for triangle in indices.as_chunks::<3>().0 {
        let mut points = [DVec3::ZERO; 3];
        for (corner, index) in triangle.iter().enumerate() {
            points[corner] = *positions.get(*index as usize)?;
        }
        let normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .try_normalize()?;
        let angle_at = |corner: usize| {
            let (left, right) = match corner {
                0 => (points[1] - points[0], points[2] - points[0]),
                1 => (points[0] - points[1], points[2] - points[1]),
                _ => (points[0] - points[2], points[1] - points[2]),
            };
            left.angle_between(right)
        };
        face_normals.push(normal);
        corner_angles.push([angle_at(0), angle_at(1), angle_at(2)]);
    }

    let mut incidents: Vec<Vec<Incident>> = vec![Vec::new(); positions.len()];
    for (triangle, triangle_indices) in indices.as_chunks::<3>().0.iter().enumerate() {
        for corner in 0..3 {
            incidents[triangle_indices[corner] as usize].push(Incident {
                triangle,
                corner,
                others: [
                    triangle_indices[(corner + 1) % 3],
                    triangle_indices[(corner + 2) % 3],
                ],
                angle: corner_angles[triangle][corner],
            });
        }
    }

    // The tie makes an edge at the sharp angle a crease. The threshold is the
    // same for every vertex, so it is computed once rather than in the loops.
    let smooth_below = sharp_angle - SHARP_ANGLE_TIE;
    let mut normals = vec![[DVec3::ZERO; 3]; triangle_count];
    for list in &incidents {
        if list.is_empty() {
            continue;
        }
        let mut parent = smoothing_groups(list, &face_normals, smooth_below);
        let mut sums: HashMap<usize, DVec3> = HashMap::new();
        for (local, incident) in list.iter().enumerate() {
            let root = find(&mut parent, local);
            *sums.entry(root).or_default() += face_normals[incident.triangle] * incident.angle;
        }
        for (local, incident) in list.iter().enumerate() {
            let root = find(&mut parent, local);
            let summed = sums.get(&root).copied().unwrap_or(DVec3::ZERO);
            normals[incident.triangle][incident.corner] = summed
                .try_normalize()
                .unwrap_or(face_normals[incident.triangle]);
        }
    }

    Some(normals)
}

/// Union the incident triangles that smooth across a shared edge at one vertex.
/// `smooth_below` is the sharp angle less [`SHARP_ANGLE_TIE`]; an edge at or
/// above it stays a crease. The returned array is the union-find forest; call
/// [`find`] to read a root.
fn smoothing_groups(list: &[Incident], face_normals: &[DVec3], smooth_below: f64) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..list.len()).collect();
    let mut by_other: HashMap<u32, Vec<usize>> = HashMap::new();
    for (local, incident) in list.iter().enumerate() {
        for other in incident.others {
            by_other.entry(other).or_default().push(local);
        }
    }
    for sharing in by_other.values() {
        // More than two triangles on one edge is non-manifold: join none of them.
        if let [first, second] = sharing[..] {
            let angle = face_normals[list[first].triangle]
                .angle_between(face_normals[list[second].triangle]);
            if angle.to_degrees() < smooth_below {
                union(&mut parent, first, second);
            }
        }
    }
    parent
}

/// The root of `node`'s smoothing group, compressing the path as it goes.
fn find(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

/// Merge the smoothing groups of two incident triangles.
fn union(parent: &mut [usize], left: usize, right: usize) {
    let (left, right) = (find(parent, left), find(parent, right));
    if left != right {
        parent[right] = left;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube centred on the origin as eight shared positions and twelve
    /// outward-wound triangles. The winding is asserted against the centre.
    fn unit_cube() -> (Vec<DVec3>, Vec<u32>) {
        let h = 0.5;
        let positions = vec![
            DVec3::new(-h, -h, -h),
            DVec3::new(h, -h, -h),
            DVec3::new(h, h, -h),
            DVec3::new(-h, h, -h),
            DVec3::new(-h, -h, h),
            DVec3::new(h, -h, h),
            DVec3::new(h, h, h),
            DVec3::new(-h, h, h),
        ];
        let indices = vec![
            4, 5, 6, 4, 6, 7, // +Z
            0, 3, 2, 0, 2, 1, // -Z
            1, 2, 6, 1, 6, 5, // +X
            0, 4, 7, 0, 7, 3, // -X
            3, 7, 6, 3, 6, 2, // +Y
            0, 1, 5, 0, 5, 4, // -Y
        ];
        for triangle in indices.as_chunks::<3>().0 {
            let normal = triangle_normal(&positions, triangle);
            let outward = (positions[triangle[0] as usize]
                + positions[triangle[1] as usize]
                + positions[triangle[2] as usize])
                .normalize();
            assert!(normal.dot(outward) > 0.9, "each face must wind outward");
        }
        (positions, indices)
    }

    /// A regular prism about the Y axis: a ring of `sides` vertices on each cap,
    /// a centre vertex on each cap, and outward-wound side and cap triangles.
    fn prism(sides: u32) -> (Vec<DVec3>, Vec<u32>) {
        let mut positions = Vec::new();
        for y in [-1.0, 1.0] {
            for i in 0..sides {
                let theta = std::f64::consts::TAU * f64::from(i) / f64::from(sides);
                positions.push(DVec3::new(theta.cos(), y, theta.sin()));
            }
        }
        let bottom_centre = u32::try_from(positions.len()).expect("fits in u32");
        positions.push(DVec3::new(0.0, -1.0, 0.0));
        let top_centre = u32::try_from(positions.len()).expect("fits in u32");
        positions.push(DVec3::new(0.0, 1.0, 0.0));

        let mut indices = Vec::new();
        for i in 0..sides {
            let next = (i + 1) % sides;
            let bottom = i;
            let bottom_next = next;
            let top = sides + i;
            let top_next = sides + next;
            indices.extend_from_slice(&[bottom, top_next, bottom_next, bottom, top, top_next]);
            indices.extend_from_slice(&[bottom_centre, bottom, bottom_next]);
            indices.extend_from_slice(&[top_centre, top_next, top]);
        }
        (positions, indices)
    }

    /// The unit face normal of one triangle, with outward winding assumed.
    fn triangle_normal(positions: &[DVec3], triangle: &[u32]) -> DVec3 {
        let points = [
            positions[triangle[0] as usize],
            positions[triangle[1] as usize],
            positions[triangle[2] as usize],
        ];
        (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize()
    }

    #[test]
    fn cube_at_45_degrees_is_faceted() {
        let (positions, indices) = unit_cube();
        let normals = shade_normals(&positions, &indices, 45.0).expect("valid cube");
        for (triangle, corners) in indices.as_chunks::<3>().0.iter().enumerate() {
            let face = triangle_normal(&positions, corners);
            for normal in normals[triangle] {
                assert!(normal.abs_diff_eq(face, 1e-12));
            }
        }
    }

    #[test]
    fn cube_at_180_degrees_is_smooth() {
        let (positions, indices) = unit_cube();
        let normals = shade_normals(&positions, &indices, 180.0).expect("valid cube");
        for (triangle, corners) in indices.as_chunks::<3>().0.iter().enumerate() {
            for (corner, vertex) in corners.iter().enumerate() {
                let radial = positions[*vertex as usize].normalize();
                assert!(normals[triangle][corner].abs_diff_eq(radial, 1e-12));
            }
        }
    }

    #[test]
    fn cube_at_90_degrees_is_still_faceted() {
        let (positions, indices) = unit_cube();
        let normals = shade_normals(&positions, &indices, 90.0).expect("valid cube");
        for (triangle, corners) in indices.as_chunks::<3>().0.iter().enumerate() {
            let face = triangle_normal(&positions, corners);
            for normal in normals[triangle] {
                assert!(normal.abs_diff_eq(face, 1e-12));
            }
        }
    }

    #[test]
    fn an_edge_at_the_sharp_angle_is_a_crease() {
        use std::f64::consts::FRAC_1_SQRT_2;
        // A flat quad in the XY plane and a second quad sharing its +X edge,
        // folded over it so that the two face normals meet at exactly 45
        // degrees. The trig constant is exact enough that the angle is the tie.
        let positions = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(1.0 - FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2),
            DVec3::new(1.0 - FRAC_1_SQRT_2, 1.0, FRAC_1_SQRT_2),
        ];
        let indices = vec![0, 1, 2, 0, 2, 3, 1, 2, 5, 1, 5, 4];
        let tilted = (positions[2] - positions[1])
            .cross(positions[5] - positions[1])
            .normalize();
        assert!(
            (tilted.dot(DVec3::Z).acos().to_degrees() - 45.0).abs() < 1e-12,
            "the second quad must meet the first at forty-five degrees"
        );

        // At the sharp angle the shared edge is a crease: the flat quad keeps
        // its own normal everywhere.
        let normals = shade_normals(&positions, &indices, 45.0).expect("valid corner");
        for corners in &normals[..2] {
            for corner in corners {
                assert!(corner.abs_diff_eq(DVec3::Z, 1e-12), "{corner}");
            }
        }

        // A hair above the sharp angle the shared edge smooths, so the corners
        // on that edge no longer wear the flat quad's normal.
        let normals = shade_normals(&positions, &indices, 45.0 + 1e-3).expect("valid corner");
        assert!(
            !normals[0][1].abs_diff_eq(DVec3::Z, 1e-12),
            "{}",
            normals[0][1]
        );
        assert!(
            !normals[0][2].abs_diff_eq(DVec3::Z, 1e-12),
            "{}",
            normals[0][2]
        );
    }

    #[test]
    fn prism_sides_are_smooth_and_caps_are_creased() {
        let (positions, indices) = prism(16);
        let normals = shade_normals(&positions, &indices, 45.0).expect("valid prism");
        for (triangle, corners) in indices.as_chunks::<3>().0.iter().enumerate() {
            let face = triangle_normal(&positions, corners);
            let is_cap = face.y.abs() > 0.5;
            for (corner, vertex) in corners.iter().enumerate() {
                let position = positions[*vertex as usize];
                let expected = if is_cap {
                    face
                } else {
                    DVec3::new(position.x, 0.0, position.z).normalize()
                };
                let tolerance = if is_cap { 1e-12 } else { 1e-9 };
                assert!(normals[triangle][corner].abs_diff_eq(expected, tolerance));
            }
        }
    }

    #[test]
    fn bad_input_is_refused() {
        let positions = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
        ];
        assert!(shade_normals(&positions, &[0, 1], 30.0).is_none());
        assert!(shade_normals(&positions, &[0, 1, 3], 30.0).is_none());
        let nan = vec![DVec3::NAN, positions[1], positions[2]];
        assert!(shade_normals(&nan, &[0, 1, 2], 30.0).is_none());
        assert!(shade_normals(&positions, &[0, 0, 0], 30.0).is_none());
        assert!(shade_normals(&positions, &[0, 1, 2], f64::NAN).is_none());
        assert_eq!(shade_normals(&positions, &[], 30.0), Some(vec![]));
    }

    #[test]
    fn every_normal_is_unit_length() {
        let (positions, indices) = prism(16);
        let normals = shade_normals(&positions, &indices, 45.0).expect("valid prism");
        for triangle in normals {
            for normal in triangle {
                assert!((normal.length() - 1.0).abs() < 1e-12);
            }
        }
    }
}
