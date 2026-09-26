//! Planting a scattered set on a mesh: from roots in UV to roots in metres.
//!
//! A [`StrandSet`] is a list of roots over *one repeat* of a material, and a
//! mesh is a list of triangles whose UV runs over however many repeats the
//! surface is. [`place`] is the join between the two: it walks the triangles,
//! finds which roots of which repeats land inside each one, and answers the
//! same strands with a position, a surface normal and a tangent frame attached.
//!
//! Nothing here is Bevy's, nothing here is `ashlar`'s, and — since the split of
//! 2026-09-20 — nothing here is the graph engine's. The triangles arrive as
//! plain `f32` arrays with their UV already divided into repeats, which is
//! decision 7 of the plan: placement needs a cross product and an interpolation
//! and no notion of a building, so it lives beside the set it plants and
//! `ashlar-bevy` stays the first crate that sees both.
//!
//! # Why the roots are bucketed rather than walked
//!
//! The obvious loop is over the lattice: a triangle covers some range of cells,
//! so walk those cells and test each one's strand. It is wrong, and it is wrong
//! for a reason the scatter introduced on purpose — the `clump` offset moves a
//! root *after* its fields are read, so a root may have left the cell its index
//! suggests, and a lattice walk would plant it twice near one edge of a tuft
//! and not at all near the other. So the roots are bucketed by where they
//! actually are: a uniform grid over `[0, 1)²`, built once per set, in which a
//! triangle looks up the buckets its own UV bounding box overlaps.
//!
//! # Which triangle owns a root on an edge
//!
//! A root on the edge shared by two triangles must be planted exactly once, and
//! the rule that decides which of them plants it has to be *local*: whether a
//! triangle owns a point cannot depend on what some other triangle did, because
//! two triangles may share a UV edge and be nowhere near each other. A box
//! projection is the ordinary case — all six faces of a cube map onto the same
//! UV square — and a rule that let the first face claim a root would leave the
//! other five bald.
//!
//! The classic local answer is a top-left fill rule, which is free and which
//! quietly depends on the two triangles winding the shared edge in *opposite*
//! directions. A UV seam that mirrors the mapping winds them the same way, and
//! then a fill rule plants the edge twice or drops it.
//!
//! So the rule here is the same idea made winding-independent. An edge is
//! oriented canonically — from its lexicographically smaller endpoint to its
//! larger, which is a property of the two points and not of either triangle —
//! and the triangle that owns it is the one whose opposite corner lies to the
//! left of that direction. Two triangles sharing an edge have their third
//! corners on opposite sides of it, so exactly one of them owns it, whichever
//! way round either was wound. The inside test is inclusive to within
//! [`INSIDE_TOLERANCE`] so that floating-point noise near the edge falls into
//! the rule rather than past it.
//!
//! What this does not decide is a root landing exactly on a shared *vertex*,
//! where the point is on two edges of every triangle around it and the two tests
//! need not agree. A jittered root hitting a vertex of the mesh exactly has
//! probability zero, and the failure if one did would be one blade too many or
//! one too few.

use glam::{Vec2, Vec3};

use crate::{Strand, StrandSet};

/// How near a triangle's edge, in barycentric units, a root is treated as being
/// *on* it.
///
/// A shared edge is the whole reason for a tolerance. The two triangles either
/// side of it compute the same point from different vertices in a different
/// order, so one of them can answer a barycentric of `-1e-9` for a root the
/// other answers `+1e-9` for. An exact test would give that root to both or to
/// neither depending on which way the rounding went; a band around zero hands it
/// to the edge rule instead, which answers the same way for both.
///
/// A hundred-thousandth of a triangle is under a tenth of a millimetre across a
/// metre of surface, which is a twentieth of the width of the blades this exists
/// to plant.
const INSIDE_TOLERANCE: f32 = 1e-5;

/// The smallest UV area, in square repeats, a triangle may have and still be
/// planted.
///
/// Below this the UV mapping has no inverse: the tangent frame is a division by
/// that area, and a triangle that is a line in UV has no "which way is `u`" to
/// answer. Those triangles exist in real meshes — a degenerate corner, a
/// collapsed cap — and they carry no area for a strand to stand on either, so
/// they are passed over rather than refused.
const MIN_UV_AREA: f32 = 1e-12;

/// The most repeats of the material one triangle's UV footprint may span, per
/// axis.
///
/// Not a budget: a wall genuinely tiling a thousand times across is a wall this
/// should plant. It is a guard against arithmetic, because the loop below is
/// over integers derived from `f32` UVs, and a mapping that went wrong by a
/// factor of a million would otherwise ask it to count to a million. A triangle
/// past this is passed over, because a footprint that large is a mapping
/// mistake rather than a surface somebody meant to grow grass on.
const MAX_REPEAT_SPAN: i64 = 1 << 12;

/// The triangles a strand set is planted on.
///
/// Parallel arrays rather than a vertex struct, because that is the shape every
/// mesh in this stack already has and the shape a caller can hand over without
/// copying. The four are read together and are expected to agree: a vertex
/// index past the end of any of them ends the triangle it appears in, so a
/// mismatched surface plants fewer strands rather than panicking.
///
/// The UVs are in **repeats of the material**, not in metres and not in the
/// mesh's own units. That conversion belongs to whoever knows the material's
/// `tile_metres`, which is a crate above this one; here, `1.0` is one repeat and
/// the seam is at every integer.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceTriangles<'a> {
    /// Vertex positions, in metres.
    pub positions: &'a [[f32; 3]],
    /// Unit shading normals, one per position.
    pub normals: &'a [[f32; 3]],
    /// Texture coordinates in repeats, one per position.
    pub uvs: &'a [[f32; 2]],
    /// Three vertex indices per triangle.
    pub indices: &'a [u32],
}

/// One strand of a set, rooted on a triangle.
///
/// A set is a list over one repeat, and a surface may reach a repeat several
/// times — six faces of a box projected onto one UV square are the ordinary
/// case — so one strand of a set becomes as many planted strands as there are
/// pieces of surface over its root.
///
/// The strand itself is carried whole rather than unpacked, because everything
/// on it is still true: the length is still the length, the colours are still
/// the colours, and only *where* it is has been answered. What placement adds
/// is that answer — a point, a normal, and the frame that makes the strand's
/// own `direction` mean the same thing on a wall as on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedStrand {
    /// The strand this was planted from, fields and all.
    pub strand: Strand,
    /// Where the root sits, in the surface's own coordinates.
    pub position: [f32; 3],
    /// The surface normal there, interpolated from the triangle's own and
    /// normalised.
    pub normal: [f32; 3],
    /// Which way `+u` runs across the surface at the root: unit length, and
    /// perpendicular to [`Self::normal`].
    ///
    /// The *direction* of the UV gradient and not its magnitude. A strand's
    /// length and width are metres, and dividing them by however many metres a
    /// repeat covers would make a blade on a finely tiled wall shorter than the
    /// same blade on a coarsely tiled one.
    pub tangent: [f32; 3],
    /// Which way `+v` runs, completing a right-handed frame with the other two.
    pub bitangent: [f32; 3],
    /// Which repeat of the material the root landed in, as the integer part of
    /// its UV.
    ///
    /// What a caller chunks by: strands of one repeat are near each other on
    /// the surface, so a repeat is a cull volume and a level-of-detail unit
    /// without anyone building a second spatial index.
    pub repeat: [i32; 2],
}

/// Plant a strand set on a mesh, keeping the strands a level of detail keeps.
///
/// `keep` is [`StrandSet::prefix`]'s cut, applied once before any triangle is
/// walked: a level that keeps a quarter walks a quarter of the roots, so the
/// far levels of a chunk cost proportionally less to place and not merely less
/// to draw.
///
/// The answer is in a deterministic order — triangle by triangle, repeat by
/// repeat, bucket by bucket — so two runs over the same mesh and the same set
/// produce the same list, which is what lets a mesh built from it be compared
/// byte for byte.
///
/// ```
/// use ashlar_strands::{Strand, StrandSet, StrandShape, SurfaceTriangles, place};
///
/// // Four roots spread over one repeat. A scatter is where a real set comes
/// // from; what placement needs of it is the roots and nothing else.
/// let strands = [[0.2, 0.2], [0.8, 0.2], [0.2, 0.8], [0.8, 0.8]]
///     .map(|root| Strand { root, length: 0.1, width: 0.003, ..Default::default() })
///     .to_vec();
/// let set = StrandSet::new("blades".to_owned(), strands, [16, 16], StrandShape::default());
///
/// // One square metre of ground, mapped to exactly one repeat.
/// let placed = place(
///     &set,
///     &SurfaceTriangles {
///         positions: &[[0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
///         normals: &[[0.0, 1.0, 0.0]; 4],
///         uvs: &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
///         indices: &[0, 1, 2, 0, 2, 3],
///     },
///     1.0,
/// );
/// assert_eq!(placed.len(), set.len());
/// ```
#[must_use]
pub fn place(set: &StrandSet, surface: &SurfaceTriangles<'_>, keep: f32) -> Vec<PlacedStrand> {
    let strands = set.prefix(keep);
    if strands.is_empty() || surface.indices.len() < 3 {
        return Vec::new();
    }
    let buckets = Buckets::of(strands);
    let mut planted = Vec::new();
    for corners in surface.indices.as_chunks::<3>().0 {
        let Some(triangle) = Triangle::of(surface, corners) else {
            continue;
        };
        let Some((first, last)) = triangle.repeats() else {
            continue;
        };
        for y in first[1]..=last[1] {
            for x in first[0]..=last[0] {
                let offset = Vec2::new(as_f32(x), as_f32(y));
                triangle.sow(strands, &buckets, offset, [x, y], &mut planted);
            }
        }
    }
    planted
}

/// An integer repeat index as the coordinate it offsets.
///
/// Bounded by [`MAX_REPEAT_SPAN`] either side of the footprint, so `f32` counts
/// it exactly and the lint about a lossy cast is answered rather than allowed.
#[expect(
    clippy::cast_precision_loss,
    reason = "a repeat index is bounded by MAX_REPEAT_SPAN, which f32 counts exactly"
)]
fn as_f32(value: i32) -> f32 {
    value as f32
}

/// One triangle, in the two spaces placement needs it in.
///
/// Built once per triangle rather than per repeat, because the UV area, the
/// tangent and the bitangent are the same in every repeat the footprint covers
/// — a repeat is a translation — and a wall spanning a hundred of them would
/// otherwise solve the same two-by-two system a hundred times.
struct Triangle {
    uv: [Vec2; 3],
    position: [Vec3; 3],
    normal: [Vec3; 3],
    /// Twice the signed UV area, which is the determinant every barycentric
    /// and both gradients are divided by.
    area: f32,
    /// The direction `+u` runs in, before it is made perpendicular to a
    /// strand's own interpolated normal.
    tangent: Vec3,
    /// The direction `+v` runs in, kept only for its sign: it is what says
    /// whether the frame is right- or left-handed on this triangle.
    bitangent: Vec3,
}

impl Triangle {
    /// The triangle three indices name, or nothing where it has no UV area to
    /// plant on.
    fn of(surface: &SurfaceTriangles<'_>, corners: &[u32; 3]) -> Option<Self> {
        let mut uv = [Vec2::ZERO; 3];
        let mut position = [Vec3::ZERO; 3];
        let mut normal = [Vec3::ZERO; 3];
        for (slot, corner) in corners.iter().enumerate() {
            let corner = usize::try_from(*corner).ok()?;
            uv[slot] = Vec2::from_array(*surface.uvs.get(corner)?);
            position[slot] = Vec3::from_array(*surface.positions.get(corner)?);
            normal[slot] = Vec3::from_array(*surface.normals.get(corner)?);
        }
        if !uv.iter().all(|corner| corner.is_finite())
            || !position.iter().all(|corner| corner.is_finite())
        {
            return None;
        }
        let (du, dv) = (uv[1] - uv[0], uv[2] - uv[0]);
        let area = du.perp_dot(dv);
        if area.abs() < MIN_UV_AREA {
            return None;
        }
        // The UV gradient of the surface, which is the two-by-two system
        // `e = du * T + dv * B` solved for the pair. It is the same frame a
        // normal map is sampled in, and it is what makes `direction` mean one
        // thing everywhere: a strand leaning towards `+u` leans along the
        // material's own grain, whether that grain runs up a wall or north
        // across a field.
        let (e1, e2) = (position[1] - position[0], position[2] - position[0]);
        let tangent = (e1 * dv.y - e2 * du.y) / area;
        let bitangent = (e2 * du.x - e1 * dv.x) / area;
        Some(Self {
            uv,
            position,
            normal,
            area,
            tangent,
            bitangent,
        })
    }

    /// The first and last repeat of the material this triangle's UV bounding
    /// box reaches, or nothing where that span is not one a mapping meant.
    fn repeats(&self) -> Option<([i32; 2], [i32; 2])> {
        let low = self.uv[0].min(self.uv[1]).min(self.uv[2]).floor();
        let high = self.uv[0].max(self.uv[1]).max(self.uv[2]).floor();
        let mut first = [0_i32; 2];
        let mut last = [0_i32; 2];
        for axis in 0..2 {
            let (low, high) = (bound(low[axis])?, bound(high[axis])?);
            if high - low >= MAX_REPEAT_SPAN {
                return None;
            }
            first[axis] = i32::try_from(low).ok()?;
            last[axis] = i32::try_from(high).ok()?;
        }
        Some((first, last))
    }

    /// Every root of `strands` inside this triangle's copy in one repeat.
    fn sow(
        &self,
        strands: &[Strand],
        buckets: &Buckets,
        offset: Vec2,
        repeat: [i32; 2],
        planted: &mut Vec<PlacedStrand>,
    ) {
        let local = self.uv.map(|corner| corner - offset);
        let low = local[0].min(local[1]).min(local[2]);
        let high = local[0].max(local[1]).max(local[2]);
        let Some((first, last)) = buckets.overlapping(low, high) else {
            return;
        };
        for y in first[1]..=last[1] {
            for x in first[0]..=last[0] {
                for index in buckets.at(x, y) {
                    let Some(strand) = strands.get(*index as usize) else {
                        continue;
                    };
                    let Some(weights) = self.inside(&local, Vec2::from_array(strand.root)) else {
                        continue;
                    };
                    planted.push(self.plant(*strand, weights, repeat));
                }
            }
        }
    }

    /// The barycentric weights of a point this triangle's copy at `local` owns,
    /// or nothing where the point is outside it or belongs to its neighbour.
    ///
    /// Weight `i` is zero exactly on the edge opposite corner `i`, so a weight
    /// inside the tolerance band is a point on that edge and is handed to
    /// [`Self::owns`]. A point on two edges at once is a point at a corner, and
    /// has to satisfy both.
    fn inside(&self, local: &[Vec2; 3], point: Vec2) -> Option<[f32; 3]> {
        let (du, dv) = (local[1] - local[0], local[2] - local[0]);
        let to = point - local[0];
        let along_v = to.perp_dot(dv) / self.area;
        let along_u = du.perp_dot(to) / self.area;
        let at_origin = 1.0 - along_v - along_u;
        let weights = [at_origin, along_v, along_u];
        for (corner, weight) in weights.iter().enumerate() {
            if *weight < -INSIDE_TOLERANCE {
                return None;
            }
            if *weight <= INSIDE_TOLERANCE && !owns(local, corner) {
                return None;
            }
        }
        Some(weights)
    }

    /// One root, with the surface interpolated at it.
    fn plant(&self, strand: Strand, weights: [f32; 3], repeat: [i32; 2]) -> PlacedStrand {
        let mut position = Vec3::ZERO;
        let mut normal = Vec3::ZERO;
        for (slot, weight) in weights.into_iter().enumerate() {
            position += self.position[slot] * weight;
            normal += self.normal[slot] * weight;
        }
        // A shading normal is a vertex attribute and may be anything; the
        // geometric normal is what the triangle itself says, and is the honest
        // answer when the attribute was zero or was never written.
        let normal = normal.normalize_or(
            (self.position[1] - self.position[0])
                .cross(self.position[2] - self.position[0])
                .normalize_or(Vec3::Y),
        );
        // The gradient made perpendicular to *this root's* normal rather than
        // to the triangle's, so a smooth-shaded surface turns its strands with
        // its shading and does not fan them at every edge.
        let tangent = (self.tangent - normal * normal.dot(self.tangent))
            .normalize_or(normal.any_orthonormal_vector());
        let bitangent = normal.cross(tangent);
        let bitangent = if bitangent.dot(self.bitangent) < 0.0 {
            -bitangent
        } else {
            bitangent
        };
        PlacedStrand {
            strand,
            position: position.to_array(),
            normal: normal.to_array(),
            tangent: tangent.to_array(),
            bitangent: bitangent.to_array(),
            repeat,
        }
    }
}

/// Whether the triangle at `local` owns the edge opposite `corner`.
///
/// The edge is oriented from its lexicographically smaller endpoint to its
/// larger — a property of the two points alone, which is what makes this
/// independent of how either triangle was wound — and the owner is the triangle
/// whose opposite corner lies to the left of that direction. The two triangles
/// either side of an edge have their opposite corners on opposite sides of it,
/// so exactly one of them answers yes.
///
/// A corner exactly *on* the edge is a triangle with no area, which
/// [`Triangle::of`] has already passed over, so the zero case does not arise
/// here; it answers no if it ever did.
fn owns(local: &[Vec2; 3], corner: usize) -> bool {
    let (a, b) = (local[(corner + 1) % 3], local[(corner + 2) % 3]);
    let (from, to) = if (b.x, b.y) < (a.x, a.y) {
        (b, a)
    } else {
        (a, b)
    };
    (to - from).perp_dot(local[corner] - from) > 0.0
}

/// One edge of a UV bounding box as the repeat index it falls in, or nothing
/// where it is not a number a repeat index is made of.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value was floored and bounded by MAX_REPEAT_SPAN before the cast"
)]
fn bound(value: f32) -> Option<i64> {
    if !value.is_finite() || value.abs() > span() {
        return None;
    }
    Some(value as i64)
}

/// [`MAX_REPEAT_SPAN`] as the float a UV coordinate is compared against.
#[expect(
    clippy::cast_precision_loss,
    reason = "MAX_REPEAT_SPAN is a small power of two, which f32 counts exactly"
)]
fn span() -> f32 {
    MAX_REPEAT_SPAN as f32
}

/// The roots of one set, in a uniform grid over `[0, 1)²`.
///
/// Compressed-row rather than a vector of vectors: a lawn is sixty-five
/// thousand roots over as many buckets, and an allocation per bucket is sixty-
/// five thousand allocations to answer a question that is two integer ranges
/// wide. The build is a counting sort, so it is one pass to count, one to place,
/// and no comparison anywhere.
struct Buckets {
    /// Buckets per axis. About one root per bucket, so that a triangle the size
    /// of a cell tests a handful of roots rather than all of them.
    cells: u32,
    /// Where each bucket's slice of [`Self::items`] starts, with a final entry
    /// holding the length.
    starts: Vec<u32>,
    /// Strand indices, grouped by bucket.
    items: Vec<u32>,
}

impl Buckets {
    /// Bucket every root of `strands` by where it actually is.
    ///
    /// By its root and not by its cell, which is the whole point: the scatter's
    /// `clump` offset moves a root after its fields are read, so the cell a
    /// strand's index names is not the cell it stands in.
    fn of(strands: &[Strand]) -> Self {
        let cells = grid(strands.len());
        let buckets = (cells as usize) * (cells as usize);
        // One extra slot, so that the prefix sum below fills `starts[bucket + 1]`
        // and the last entry is the total.
        let mut starts = vec![0_u32; buckets + 1];
        for strand in strands {
            starts[bucket(strand.root, cells) + 1] += 1;
        }
        for index in 1..starts.len() {
            starts[index] += starts[index - 1];
        }
        let mut cursor = starts.clone();
        let mut items = vec![0_u32; strands.len()];
        for (index, strand) in strands.iter().enumerate() {
            let slot = &mut cursor[bucket(strand.root, cells)];
            items[*slot as usize] = u32::try_from(index).unwrap_or(u32::MAX);
            *slot += 1;
        }
        Self {
            cells,
            starts,
            items,
        }
    }

    /// The first and last bucket a UV bounding box overlaps, or nothing where
    /// the box misses `[0, 1)²` entirely.
    ///
    /// The box is the triangle's, already brought into one repeat, so a
    /// triangle that straddles a seam is clipped here and met again in the
    /// neighbouring repeat, where the other half of it is inside.
    fn overlapping(&self, low: Vec2, high: Vec2) -> Option<([u32; 2], [u32; 2])> {
        if high.x < 0.0 || high.y < 0.0 || low.x > 1.0 || low.y > 1.0 {
            return None;
        }
        Some((
            [axis(low.x, self.cells), axis(low.y, self.cells)],
            [axis(high.x, self.cells), axis(high.y, self.cells)],
        ))
    }

    /// The strand indices in one bucket.
    fn at(&self, x: u32, y: u32) -> &[u32] {
        let index = (y as usize) * (self.cells as usize) + (x as usize);
        let (Some(first), Some(last)) = (self.starts.get(index), self.starts.get(index + 1)) else {
            return &[];
        };
        &self.items[*first as usize..*last as usize]
    }
}

/// The most buckets per axis the lookup grid is built with.
///
/// Four thousand and ninety-six, which is `ashlar_material`'s own `MAX_PERIOD`
/// — the coarsest lattice a strand layer may be scattered on — written out here
/// rather than imported, because this crate is the half a game links and the
/// graph engine that owns the period is not in its tree. The two numbers are
/// the same number for a reason: a set has at most `MAX_PERIOD` cells on an
/// axis times its strands per cell, so a grid this size is already one bucket
/// per cell and anything finer is memory nobody asked for.
const MAX_BUCKETS: u32 = 4096;

/// How many buckets per axis a set of this size wants.
///
/// The square root, so a bucket holds about one root and a triangle tests about
/// as many roots as it can plant. Bounded above because a grid is memory a
/// scatter did not ask for, and below because a zero-cell grid has no buckets
/// to put anything in.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a root count is bounded by the lattice, and the result is clamped"
)]
fn grid(strands: usize) -> u32 {
    let count = u32::try_from(strands).unwrap_or(u32::MAX);
    (f64::from(count).sqrt().ceil() as u32).clamp(1, MAX_BUCKETS)
}

/// Which bucket a root falls in.
fn bucket(root: [f32; 2], cells: u32) -> usize {
    (axis(root[1], cells) as usize) * (cells as usize) + (axis(root[0], cells) as usize)
}

/// One coordinate as a bucket index, held inside the grid.
///
/// A root is already in `[0, 1)` — the scatter wraps it — but a *bounding box*
/// edge is not, so the clamp is what lets one function answer both.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "a grid edge is bounded by MAX_PERIOD, and the index is clamped before it is cast"
)]
fn axis(coordinate: f32, cells: u32) -> u32 {
    let scaled = coordinate * (cells as f32);
    if scaled.is_nan() {
        return 0;
    }
    (scaled.floor().clamp(0.0, f32::from(u16::MAX)) as u32).min(cells.saturating_sub(1))
}
