use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::{Axis, Geometry, Pose};

/// Default welding distance. Kernel output is exact to far better than a
/// micrometre, and no building detail is authored below it.
pub const WELD_TOLERANCE: f64 = 1e-6;

/// Which surface of an operand a face came from.
///
/// Non-exhaustive: a new kind of subtraction must not break a consumer that
/// matches on the origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum FaceOrigin {
    /// The operand's own surface.
    #[default]
    Body,
    /// A cutter subtracted from the operand; the index is the cutter's position
    /// in the operand's geometry, depth first.
    Cutter(u32),
    /// A damage solid subtracted after the fact; the index is its position in
    /// the damage list. The faces it exposes belong to no operand and carry
    /// [`FaceSource::NO_OPERAND`].
    Damage(u32),
}

/// Where one triangle came from.
///
/// The operand is an index among the geometries handed to the mesher in one
/// call, not an instance: the mesher is given geometry and nothing else, and
/// the caller holds the table from operand to whatever placed it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceSource {
    /// Index of the geometry this face came from, among those handed to the
    /// mesher in one call. Always 0 out of [`GeometryMesher::mesh`]. A face
    /// exposed by damage belongs to no operand and carries
    /// [`FaceSource::NO_OPERAND`].
    pub operand: u32,
    /// Which surface of that operand.
    pub origin: FaceOrigin,
}

impl FaceSource {
    /// The body of operand 0: what an uncut face of a lone geometry is.
    pub const BODY: Self = Self {
        operand: 0,
        origin: FaceOrigin::Body,
    };

    /// The operand of a face that belongs to no operand: one exposed by
    /// damage, which lies on the damage solid's surface and not on any member's.
    pub const NO_OPERAND: u32 = u32::MAX;

    /// Cutter `index` of operand 0.
    pub const fn cutter(index: u32) -> Self {
        Self {
            operand: 0,
            origin: FaceOrigin::Cutter(index),
        }
    }

    /// Whether the face was exposed by a subtraction: a cutter or damage.
    pub const fn is_cut(self) -> bool {
        !matches!(self.origin, FaceOrigin::Body)
    }
}

/// Renderer-independent indexed triangles. Vertices are split at shading seams,
/// UV seams and the boundary of a cut, and shared everywhere else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriangleMesh {
    /// Part-local vertex positions in metres.
    pub positions: Vec<DVec3>,
    /// Unit shading normals, one per position.
    pub normals: Vec<DVec3>,
    /// Texture coordinates in metres, one per position.
    pub uvs: Vec<[f64; 2]>,
    /// Three vertex indices per outward-wound triangle.
    pub indices: Vec<u32>,
    /// Where each triangle came from, one per triangle.
    ///
    /// Per triangle rather than per vertex, because that is what it is a fact
    /// about. A renderer that wants the cut flag per vertex — `ashlar-bevy`
    /// uploads it as a vertex attribute so a material can darken its own cut
    /// faces — gets one unambiguous flag per vertex out of
    /// [`weld`](Self::weld), which treats the edge of a cut as a seam.
    pub sources: Vec<FaceSource>,
}

// Quantization is a bucket index, and the tolerance keeps the value in range.
#[allow(clippy::cast_possible_truncation)]
fn quantize(value: f64, tolerance: f64) -> i64 {
    (value / tolerance).round() as i64
}

/// Vertices are equal only when position, normal, UV and cut provenance all
/// agree, so a crease, a UV seam or the edge of a cut still splits them.
///
/// The cut flag is in the key because it is per *triangle* and every renderer
/// that reads it reads it per vertex: `ashlar-bevy` uploads it as a vertex
/// attribute, and a vertex shared between a cut face and an uncut one would
/// have to be both. Splitting is the rule rather than taking the maximum,
/// because an interpolated maximum is a gradient across the uncut face — a
/// material asking "am I on a cut face" would get "a third of one" halfway
/// along a triangle — where a split gives every fragment of a face the flag
/// its own face has. It costs the vertices along the boundary of a cut and
/// nothing anywhere else.
///
/// The key holds the flag and not the whole source: a renderer reads the flag
/// per vertex, and nothing reads an operand per vertex, so splitting on the
/// operand would cost vertices and buy nothing.
type WeldKey = ([i64; 3], [i64; 3], [i64; 2], bool);

impl TriangleMesh {
    /// Number of triangles, which is also the length of `sources`.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Whether the triangle at `face` was exposed by a subtraction. False when
    /// `face` is out of range.
    pub fn is_cut(&self, face: usize) -> bool {
        self.sources.get(face).is_some_and(|source| source.is_cut())
    }

    /// One flag per triangle, in order: whether the triangle was exposed by a
    /// subtraction.
    pub fn cut_faces(&self) -> impl Iterator<Item = bool> + '_ {
        self.sources.iter().map(|source| source.is_cut())
    }

    /// The three positions of each triangle, in winding order.
    pub fn triangles(&self) -> impl Iterator<Item = [DVec3; 3]> + '_ {
        self.corners().map(|c| c.map(|i| self.positions[i]))
    }

    /// The three vertex indices of each triangle, for reading normals or UVs
    /// alongside positions.
    pub fn corners(&self) -> impl Iterator<Item = [usize; 3]> + '_ {
        self.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| t.map(|i| i as usize))
    }

    /// Whether the buffers agree with each other: three indices per triangle,
    /// one source per triangle, one normal and UV per position, and no index
    /// past the end.
    pub fn is_consistent(&self) -> bool {
        self.indices.len().is_multiple_of(3)
            && self.sources.len() == self.triangle_count()
            && self.normals.len() == self.positions.len()
            && self.uvs.len() == self.positions.len()
            && self
                .indices
                .iter()
                .all(|i| (*i as usize) < self.positions.len())
    }

    /// Append one triangle's own three vertices, sharing nothing. Call
    /// [`weld`](Self::weld) afterwards to share what is identical.
    pub fn push_triangle(
        &mut self,
        positions: [DVec3; 3],
        normals: [DVec3; 3],
        uvs: [[f64; 2]; 3],
        source: FaceSource,
    ) {
        for corner in 0..3 {
            // A fresh vertex per corner, so the index always fits if the mesh does.
            self.indices.push(
                u32::try_from(self.positions.len()).expect("vertex count within a 32-bit index"),
            );
            self.positions.push(positions[corner]);
            self.normals.push(normals[corner]);
            self.uvs.push(uvs[corner]);
        }
        self.sources.push(source);
    }

    /// Give every triangle its own three vertices again. Projections and other
    /// per-face work need a vertex that belongs to one triangle only.
    pub fn unweld(&mut self) {
        let mut positions = Vec::with_capacity(self.indices.len());
        let mut normals = Vec::with_capacity(self.indices.len());
        let mut uvs = Vec::with_capacity(self.indices.len());
        for &index in &self.indices {
            let index = index as usize;
            positions.push(self.positions[index]);
            normals.push(self.normals[index]);
            uvs.push(self.uvs[index]);
        }
        self.indices = (0..positions.len())
            .map(|i| u32::try_from(i).expect("vertex count within a 32-bit index"))
            .collect();
        self.positions = positions;
        self.normals = normals;
        self.uvs = uvs;
    }

    /// Share vertices that agree on position, normal, UV and cut provenance
    /// within `tolerance` metres. Welding is an optimization, not a repair: two
    /// vertices that fall either side of a bucket edge stay separate, which
    /// costs a vertex and nothing else. A non-positive tolerance leaves the mesh
    /// alone.
    ///
    /// A triangle whose corners all land in one bucket, or two of them in one
    /// and the third in another, collapses to a line and is dropped, along with
    /// its `sources` entry. Such a triangle has no area, so the surface is
    /// unchanged, which is the promise welding always made. Kernel output after
    /// a boolean near a coincident plane is exactly these slivers: edges below
    /// the tolerance are common where a cut runs almost along a face, and a
    /// triangle left with two equal indices has no frame for a later
    /// projection. A vertex the dropped triangle alone referenced stays in the
    /// buffers; it is harmless, and compacting it would cost more than it saves.
    ///
    /// Cut provenance is part of the key, so the boundary between a cut face
    /// and the face it was cut into is a seam, exactly as a crease is. That is
    /// what lets a renderer carry the per-triangle flag as per-vertex data
    /// without having to decide what a vertex on both sides of it means.
    ///
    /// ```
    /// use ashlar::{FaceSource, TriangleMesh, WELD_TOLERANCE, glam::DVec3};
    ///
    /// // Two triangles of one quad, pushed as unshared corners.
    /// let corners = [
    ///     DVec3::new(0.0, 0.0, 0.0),
    ///     DVec3::new(2.0, 0.0, 0.0),
    ///     DVec3::new(2.0, 3.0, 0.0),
    ///     DVec3::new(0.0, 3.0, 0.0),
    /// ];
    /// let mut mesh = TriangleMesh::default();
    /// for face in [[0, 1, 2], [0, 2, 3]] {
    ///     let points = face.map(|i| corners[i]);
    ///     mesh.push_triangle(points, [DVec3::Z; 3], points.map(|p| [p.x, p.y]), FaceSource::BODY);
    /// }
    /// assert_eq!(mesh.positions.len(), 6);
    ///
    /// mesh.weld(WELD_TOLERANCE);
    /// assert_eq!(mesh.positions.len(), 4, "the shared diagonal is one pair now");
    /// assert_eq!(mesh.triangle_count(), 2, "welding never changes the surface");
    /// assert!(mesh.is_consistent());
    /// ```
    pub fn weld(&mut self, tolerance: f64) {
        if tolerance <= 0.0 || !tolerance.is_finite() {
            return;
        }
        // Normals are unit vectors, so their bucket is an angle, not a length.
        let angular = 1e-4;
        let mut seen: std::collections::HashMap<WeldKey, u32> = std::collections::HashMap::new();
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::with_capacity(self.indices.len());
        let mut sources = Vec::with_capacity(self.sources.len());
        for (face, triangle) in self.indices.as_chunks::<3>().0.iter().enumerate() {
            // One flag to a triangle. A mesh whose flags are short of its faces
            // is inconsistent already, and reading `false` there welds it the
            // way this function always did rather than turning a caller's bug
            // into a panic.
            let cut = self.is_cut(face);
            let mut welded = [0u32; 3];
            for (corner, &old) in triangle.iter().enumerate() {
                let old = old as usize;
                let key = (
                    self.positions[old]
                        .to_array()
                        .map(|v| quantize(v, tolerance)),
                    self.normals[old].to_array().map(|v| quantize(v, angular)),
                    self.uvs[old].map(|v| quantize(v, tolerance)),
                    cut,
                );
                welded[corner] = *seen.entry(key).or_insert_with(|| {
                    positions.push(self.positions[old]);
                    normals.push(self.normals[old]);
                    uvs.push(self.uvs[old]);
                    u32::try_from(positions.len() - 1).expect("vertex count within a 32-bit index")
                });
            }
            if welded[0] == welded[1] || welded[1] == welded[2] || welded[0] == welded[2] {
                continue;
            }
            indices.extend_from_slice(&welded);
            if let Some(source) = self.sources.get(face) {
                sources.push(*source);
            }
        }
        self.indices = indices;
        self.sources = sources;
        self.positions = positions;
        self.normals = normals;
        self.uvs = uvs;
    }
}

/// How an element's surface is mapped into texture space. One UV unit is one
/// metre in every mode; the material's own tile size turns metres into repeats.
///
/// Non-exhaustive: a projection is a rendering detail, and a new one must not
/// break a consumer that matches on the mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum UvMode {
    /// Per-face frame from the face's own normal, with the face's own origin.
    /// Lengths are preserved on any slope, and every face starts at zero, so
    /// two coplanar faces of different elements do not share a repeat phase.
    #[default]
    Planar,
    /// World-axis mapping: the two axes the face is least perpendicular to,
    /// read straight off the part frame. Coplanar faces of separate elements
    /// therefore share one grid and their repeats line up. Faces at an angle to
    /// their dominant axis are foreshortened, as in any triplanar mapping, and
    /// a face read from the far side is mirrored.
    Box,
    /// Angle times radius along U and height along V, around one axis of the
    /// part frame. A cylinder gets metre-true repeats and exactly one seam,
    /// where the angle wraps. Faces perpendicular to the axis, such as caps,
    /// fall back to the box mapping of the plane they lie in.
    Cylindrical {
        /// Axis of revolution in part space.
        axis: Axis,
    },
}

/// A backend failure associated with a geometry expression or part element.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct MeshError {
    /// Authored location responsible for the failure.
    pub path: String,
    /// Backend diagnostic, including unsupported input and resource limits.
    pub reason: String,
}

/// One opening, as the rectangle a cutter makes through the solid it cuts, in
/// the frame of the geometry that was handed in.
///
/// An arched or round opening publishes its bounding rectangle in v1. The
/// rectangle lies at the middle of the solid's thickness, so it is the surface
/// a camera crosses when it walks through.
#[derive(Clone, Debug, PartialEq)]
pub struct PortalShape {
    /// The id the cutter was marked with.
    pub id: String,
    /// The rectangle's corners, in order around it. It lies at the middle of
    /// the solid's thickness, so it is the surface a camera crosses.
    pub corners: [DVec3; 4],
    /// Unit normal of the rectangle: the axis the cutter pierces along. Its
    /// sign carries no meaning; a portal has two sides and no front.
    pub normal: DVec3,
}

/// One geometry and where it stands in the frame the group is meshed in.
#[derive(Clone, Copy, Debug)]
pub struct PlacedGeometry<'a> {
    /// The solid, in its own frame.
    pub geometry: &'a Geometry,
    /// Applied after the geometry's own pose.
    pub pose: Pose,
}

/// One subtraction from a group, and whether what it cuts free goes too.
#[derive(Clone, Copy, Debug)]
pub struct PlacedCut<'a> {
    /// The solid, in its own frame.
    pub geometry: &'a Geometry,
    /// Applied after the geometry's own pose.
    pub pose: Pose,
    /// See [`Damage::collapse`](crate::Damage::collapse): a true cut drops the
    /// connected pieces it leaves unattached, unless the group already had them
    /// loose and the cut never touched them.
    pub collapse: bool,
}

/// What a backend answers for a group. Non-exhaustive so that what a group
/// publishes beside its surface can grow without breaking a backend.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct GroupMesh {
    /// The union's surface, in the group's frame.
    pub mesh: TriangleMesh,
}

impl GroupMesh {
    /// Build a group answer from its surface. Needed because the struct is
    /// non-exhaustive, so a backend cannot build one with a literal.
    pub fn new(mesh: TriangleMesh) -> Self {
        Self { mesh }
    }
}

/// Solid evaluation boundary. Implementations validate editable geometry before use.
pub trait GeometryMesher: Send + Sync {
    /// Evaluate an expression in its parent's frame. Empty solids return empty meshes.
    fn mesh(&self, geometry: &Geometry) -> Result<TriangleMesh, MeshError>;

    /// Union placed solids into one surface, then subtract each cut in order.
    ///
    /// Every face names its source: `operand` is the index into `solids`,
    /// `origin` is that operand's body or cutter exactly as [`mesh`](Self::mesh)
    /// would report it, and a face exposed by `cuts[j]` has origin
    /// [`FaceOrigin::Damage`] and operand [`FaceSource::NO_OPERAND`].
    ///
    /// The group's base is the lowest Y of the union's bounds before any cut.
    /// A piece is loose when the lowest Y of its bounds is more than 1e-3 m
    /// above the base. Before a cut whose `collapse` is true, the loose pieces
    /// the group already has are noted. After the cut, what is left is split
    /// into connected pieces, and a loose piece falls unless it is one of
    /// those, untouched by the cut: a piece the cut created falls, a loose
    /// piece the cut bit into falls, and a loose piece the cut never touched
    /// stays where its author put it. Cuts apply in order, so a later cut sees
    /// what an earlier collapse left. A piece resting on the base is kept even
    /// if it touches nothing else: a wall stub standing on the floor line
    /// stands.
    ///
    /// This is the rule for one group, where reaching the base and being
    /// supported are the same thing. A whole building is not this backend's:
    /// support crosses groups, because a storey is held up by the one under it,
    /// so `GroupSolids` in `ashlar-manifold` owns the building-wide rule and is
    /// what a replay of a damage log goes through.
    ///
    /// The answer is stateless: the same arguments give the same solid, which
    /// is what a loading game, a late-joining client and a server replaying a
    /// damage list rely on. The triangulation is *not* part of that promise.
    /// Solids that touch exactly face to face union with no internal wall;
    /// solids that miss by any distance stay separate components and that is
    /// not an error.
    ///
    /// The default refuses, so a backend that cannot union placed geometry
    /// keeps it and says so.
    fn mesh_group(
        &self,
        _solids: &[PlacedGeometry<'_>],
        _cuts: &[PlacedCut<'_>],
    ) -> Result<GroupMesh, MeshError> {
        Err(MeshError {
            path: "group".into(),
            reason: "this backend cannot union placed geometry".into(),
        })
    }

    /// The portals of one geometry: one per cutter marked
    /// [`Geometry::portal`], in cutter order.
    ///
    /// The default is empty for geometry that marks no portal, and refuses
    /// marked geometry because a backend that cannot derive a rectangle must
    /// not answer an empty list that reads as "this geometry has no openings".
    fn portals(&self, geometry: &Geometry) -> Result<Vec<PortalShape>, MeshError> {
        if geometry
            .cutters()
            .iter()
            .any(|cutter| cutter.portal.is_some())
        {
            Err(MeshError {
                path: "geometry".into(),
                reason: "this backend cannot derive portals".into(),
            })
        } else {
            Ok(Vec::new())
        }
    }
}

/// Project a face onto an orthonormal surface frame: one UV unit is one metre.
/// Coplanar triangles share the same frame and origin. Seams remain where the
/// dominant axis changes; this is planar mapping, not a curved-surface unwrap.
/// Returns `None` for non-finite or degenerate triangles.
pub fn planar_uvs(points: [DVec3; 3]) -> Option<[[f64; 2]; 3]> {
    if points.iter().any(|p| !p.is_finite()) {
        return None;
    }
    let normal = (points[1] - points[0])
        .cross(points[2] - points[0])
        .try_normalize()?;
    let (axis_u, axis_v) = match normal.abs().max_position() {
        0 => (DVec3::Z, DVec3::Y),
        1 => (DVec3::X, DVec3::Z),
        _ => (DVec3::X, DVec3::Y),
    };
    let u = (axis_u - normal * axis_u.dot(normal)).try_normalize()?;
    let v = (axis_v - normal * axis_v.dot(normal) - u * axis_v.dot(u)).try_normalize()?;
    Some(points.map(|p| [p.dot(u), p.dot(v)]))
}

/// Read one triangle in the two axes perpendicular to `axis`, in part space.
/// The shared origin is what makes coplanar faces of separate elements tile
/// together.
fn axis_plane_uvs(points: [DVec3; 3], axis: Axis) -> [[f64; 2]; 3] {
    // The same axis pair [`planar_uvs`] picks, so switching an element to box
    // mapping moves the origin without also turning the texture.
    let (u, v) = match axis {
        Axis::X => (Axis::Z, Axis::Y),
        Axis::Y => (Axis::X, Axis::Z),
        Axis::Z => (Axis::X, Axis::Y),
    };
    points.map(|p| [p[u.index()], p[v.index()]])
}

/// Project a face onto world axes rather than its own frame; see [`UvMode::Box`].
/// Returns `None` for non-finite or degenerate triangles.
pub fn box_uvs(points: [DVec3; 3]) -> Option<[[f64; 2]; 3]> {
    Some(axis_plane_uvs(points, dominant_axis(points)?))
}

/// Wrap a face around an axis; see [`UvMode::Cylindrical`]. Returns `None` for
/// non-finite or degenerate triangles.
pub fn cylindrical_uvs(points: [DVec3; 3], axis: Axis) -> Option<[[f64; 2]; 3]> {
    let normal = face_normal(points)?;
    if normal.dot(axis.unit()).abs() > 0.7 {
        return Some(axis_plane_uvs(points, axis));
    }
    let (u, v) = axis.tangents();
    let angle = |p: DVec3| p[v.index()].atan2(p[u.index()]);
    let radius = |p: DVec3| DVec3::new(p[u.index()], p[v.index()], 0.0).length();
    // One seam: every corner is unwrapped into the half turn either side of the
    // first, so a triangle spanning the wrap does not stretch across the solid.
    let first = angle(points[0]);
    let mean = points.iter().map(|p| radius(*p)).sum::<f64>() / 3.0;
    let uvs = points.map(|p| {
        let mut turn = angle(p) - first;
        turn -= std::f64::consts::TAU * (turn / std::f64::consts::TAU).round();
        [(first + turn) * mean, p[axis.index()]]
    });
    uvs.iter()
        .flatten()
        .all(|value| value.is_finite())
        .then_some(uvs)
}

fn face_normal(points: [DVec3; 3]) -> Option<DVec3> {
    if points.iter().any(|p| !p.is_finite()) {
        return None;
    }
    (points[1] - points[0])
        .cross(points[2] - points[0])
        .try_normalize()
}

fn dominant_axis(points: [DVec3; 3]) -> Option<Axis> {
    Some(match face_normal(points)?.abs().max_position() {
        0 => Axis::X,
        1 => Axis::Y,
        _ => Axis::Z,
    })
}

/// Project one triangle under the given mode.
pub fn project_uvs(points: [DVec3; 3], mode: UvMode) -> Option<[[f64; 2]; 3]> {
    match mode {
        UvMode::Planar => planar_uvs(points),
        UvMode::Box => box_uvs(points),
        UvMode::Cylindrical { axis } => cylindrical_uvs(points, axis),
    }
}

impl TriangleMesh {
    /// Replace every UV using `mode`, after the solid has been evaluated.
    /// Returns false, leaving the mesh untouched, if a triangle is degenerate.
    /// Replace every UV using `mode`, after the solid has been evaluated. A
    /// projection is per face, so the mesh is unwelded first and is left that
    /// way: weld it again once the UVs are final.
    /// Returns false, leaving the mesh in an unspecified but consistent state,
    /// if a triangle is degenerate.
    pub fn project_uvs(&mut self, mode: UvMode) -> bool {
        self.unweld();
        let mut uvs = Vec::with_capacity(self.positions.len());
        for triangle in self.positions.as_chunks::<3>().0 {
            let Some(projected) = project_uvs(*triangle, mode) else {
                return false;
            };
            uvs.extend_from_slice(&projected);
        }
        self.uvs = uvs;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_uvs_share_coordinates_across_a_sloped_quad_diagonal() {
        let a = DVec3::new(1.0, 2.0, 3.0);
        let b = a + DVec3::new(4.0, 1.0, 0.0);
        let c = a + DVec3::new(0.0, 2.0, 5.0);
        let d = b + c - a;
        let first = planar_uvs([a, b, c]).expect("first valid triangle");
        let second = planar_uvs([b, d, c]).expect("second valid triangle");
        for (left, right) in [(first[1], second[0]), (first[2], second[2])] {
            assert!(glam::DVec2::from_array(left).distance(glam::DVec2::from_array(right)) < 1e-12);
        }
        assert!(planar_uvs([a; 3]).is_none());
        assert!(planar_uvs([DVec3::NAN, b, c]).is_none());
    }

    /// Two triangles of one quad, the second of them cut. They share the
    /// diagonal geometrically and must not share a vertex, because the flag a
    /// renderer reads off that vertex would have to be both values at once.
    #[test]
    fn welding_keeps_a_cut_face_off_the_vertices_of_the_face_beside_it() {
        let corners = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 3.0, 0.0),
            DVec3::new(0.0, 3.0, 0.0),
        ];
        let build = |second_is_cut: bool| {
            let mut mesh = TriangleMesh::default();
            for (face, cut) in [([0, 1, 2], false), ([0, 2, 3], second_is_cut)] {
                let points = face.map(|i| corners[i]);
                let source = if cut {
                    FaceSource::cutter(0)
                } else {
                    FaceSource::BODY
                };
                mesh.push_triangle(points, [DVec3::Z; 3], points.map(|p| [p.x, p.y]), source);
            }
            mesh.weld(WELD_TOLERANCE);
            mesh
        };

        let uncut = build(false);
        assert_eq!(uncut.positions.len(), 4, "the shared diagonal is one pair");

        let mixed = build(true);
        assert_eq!(
            mixed.positions.len(),
            6,
            "the diagonal is a seam once the faces either side of it differ"
        );
        assert_eq!(
            mixed.triangle_count(),
            2,
            "welding never changes the surface"
        );
        assert!(mixed.is_consistent());
        // Every vertex belongs to faces of one kind, which is the property the
        // vertex attribute rests on.
        for (face, corners) in mixed.corners().enumerate() {
            for corner in corners {
                let kinds: Vec<bool> = mixed
                    .corners()
                    .enumerate()
                    .filter(|(_, other)| other.contains(&corner))
                    .map(|(other, _)| mixed.is_cut(other))
                    .collect();
                assert!(
                    kinds.iter().all(|kind| *kind == mixed.is_cut(face)),
                    "vertex {corner} is shared between a cut face and an uncut one"
                );
            }
        }
    }

    #[test]
    fn a_source_is_cut_unless_it_is_the_body() {
        assert!(!FaceSource::BODY.is_cut());
        let cutter = FaceSource::cutter(3);
        assert!(cutter.is_cut());
        assert_eq!(cutter.operand, 0);
        assert_eq!(cutter.origin, FaceOrigin::Cutter(3));
        let damage = FaceSource {
            operand: 0,
            origin: FaceOrigin::Damage(0),
        };
        assert!(damage.is_cut());
        // Out of range: nothing was pushed, so nothing was cut.
        assert!(!TriangleMesh::default().is_cut(0));
    }

    /// A sliver with a 1e-8 edge lands in one bucket at [`WELD_TOLERANCE`] and
    /// collapses; dropping it must take its source with it and leave the two
    /// sound neighbours, in order.
    #[test]
    fn welding_drops_a_triangle_it_collapsed_and_keeps_sources_in_step() {
        let push = |mesh: &mut TriangleMesh, points: [DVec3; 3], source: FaceSource| {
            mesh.push_triangle(points, [DVec3::Z; 3], points.map(|p| [p.x, p.y]), source);
        };
        let mut mesh = TriangleMesh::default();
        push(
            &mut mesh,
            [
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
            ],
            FaceSource::BODY,
        );
        push(
            &mut mesh,
            [
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(1.0 + 1e-8, 0.0, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
            ],
            FaceSource::cutter(0),
        );
        push(
            &mut mesh,
            [
                DVec3::new(1.0, 1.0, 0.0),
                DVec3::new(1.0 + 1e-8, 0.0, 0.0),
                DVec3::new(2.0, 1.0, 0.0),
            ],
            FaceSource::cutter(1),
        );

        mesh.weld(WELD_TOLERANCE);

        assert_eq!(
            mesh.triangle_count(),
            2,
            "the sliver has no area and is gone"
        );
        assert!(mesh.is_consistent());
        assert_eq!(
            mesh.sources,
            vec![FaceSource::BODY, FaceSource::cutter(1)],
            "the surviving sources stay in order"
        );
    }
}
