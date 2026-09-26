//! The mesh: from a planted strand to the triangles that draw it.
//!
//! One strand is a quadratic Bézier from its root, sampled into `segments + 1`
//! rings, and one of two shapes wrapped around those rings —
//! [`StrandProfile::Blade`] is a tapered ribbon and [`StrandProfile::Fibre`] a
//! three-sided tube. What comes out is a [`StrandMesh`]: plain vectors of plain
//! arrays, because this crate has no renderer in it and the attribute names
//! belong to whoever uploads them.
//!
//! # Why a quadratic Bézier
//!
//! Because three points are exactly what a strand carries. The root is the
//! first, [`Strand::lean`](crate::Strand::lean) tilts the second away from the
//! surface normal, and [`Strand::bend`](crate::Strand::bend) turns the third
//! further in the same plane. A cubic would need a fourth control the scatter
//! has no field for, and a circular arc would make `bend` an angle that stops
//! meaning anything past a half turn. It also gives the tip a closed form —
//! [`tip_of`] — which is a thing a test can check without sampling a curve.
//!
//! The lean plane is spanned by the surface normal and the strand's own
//! `direction`, mapped through the tangent frame [`PlacedStrand`] carries. Both
//! Bézier tangents lie in that plane, so the vector perpendicular to it is
//! perpendicular to the whole curve, and *that* is what the width is measured
//! along. It costs one cross product per strand instead of one per ring, and it
//! keeps a blade from twisting along its own length.
//!
//! # Why the normals are not the geometry's
//!
//! A blade is a flat ribbon, and a flat ribbon shades like a flat ribbon: one
//! constant normal across its width, with a hard edge where the light stops.
//! Real grass is a shallow channel, so the normals here are splayed outwards
//! across the width by [`BLADE_CAMBER`] — the geometry stays two triangles per
//! segment and the shading is a curved blade. This is the same bargain
//! [`StrandShape::root_occlusion`] makes one level down: the cheap gradient is
//! most of what the eye reads.

use glam::Vec3;

use crate::{MAX_SEGMENTS, PlacedStrand, StrandProfile};

/// How far a blade's normals are splayed out across its width, in radians.
///
/// About seventeen degrees each way. Enough that a blade lit from one side has
/// a bright edge and a dark one rather than one flat tone, and little enough
/// that the two edges still read as the same surface. It is a constant rather
/// than a layer field because it is a property of *being a blade* — a ribbon
/// with no camber at all is a strip of paper, and one with a lot of it is a
/// tube, which is what [`StrandProfile::Fibre`] is for.
pub const BLADE_CAMBER: f32 = 0.3;

/// The shape constants a mesh is built to, as the layer declared them.
///
/// Carried apart from the strands because they are the same for every strand of
/// a layer: sixty-five thousand copies of one segment count is sixty-five
/// thousand words of nothing. [`StrandSet::shape`](crate::StrandSet::shape) is
/// where one comes from, and the fields are public so that a caller building a
/// cheaper level of detail can lower the segment count without re-scattering
/// anything.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrandShape {
    /// Which shape a strand is built as.
    pub profile: StrandProfile,
    /// How many segments the curve is sampled into, held to
    /// `1..=`[`MAX_SEGMENTS`].
    pub segments: u32,
    /// How much of the root width the tip gives up, in `0..=1`.
    pub taper: f32,
    /// How much the vertex colour darkens at the root, in `0..=1`.
    pub root_occlusion: f32,
    /// Where along the strand it is widest, in `0..=1`.
    pub midpoint: f32,
}

impl Default for StrandShape {
    fn default() -> Self {
        Self {
            profile: StrandProfile::Blade,
            segments: 3,
            taper: 1.0,
            root_occlusion: 0.0,
            midpoint: 0.0,
        }
    }
}

impl StrandShape {
    /// The segment count this shape is actually built at, held to the range a
    /// layer is validated against.
    ///
    /// A [`StrandSet`](crate::StrandSet) carries a count validation already
    /// refused anything outside; this type does not, because a level of detail
    /// is a caller lowering the count by hand and a zero there should be one
    /// segment rather than an empty mesh.
    pub fn segments(&self) -> u32 {
        self.segments.clamp(1, MAX_SEGMENTS)
    }

    /// How many vertices one strand of this shape is built from.
    ///
    /// A ring per segment boundary, and two vertices per ring for a ribbon or
    /// three for a tube. Public because a caller sizing a buffer should not
    /// have to build the mesh to find out how big it is.
    pub fn vertices_per_strand(&self) -> usize {
        (self.segments() as usize + 1) * self.profile.ring()
    }

    /// How many triangles one strand of this shape is built from.
    ///
    /// Two per segment for a ribbon, six for a tube — one quad per face, and a
    /// tube has three faces where a ribbon has one.
    ///
    /// A fully tapered ribbon spends one of its two tip triangles on a quad
    /// that has collapsed to a triangle, which is a degenerate the rasteriser
    /// discards for nothing. The alternative is an index buffer whose shape
    /// depends on the taper, and a uniform one is worth more than a triangle
    /// per blade.
    pub fn triangles_per_strand(&self) -> usize {
        self.segments() as usize * self.profile.faces() * 2
    }
}

impl StrandProfile {
    /// How many vertices one ring of this profile carries.
    fn ring(self) -> usize {
        match self {
            Self::Blade => 2,
            Self::Fibre => 3,
        }
    }

    /// How many quads one segment of this profile is wrapped in.
    ///
    /// A ribbon has one face and two edges; a tube has three of each. This is
    /// what the two differ by, and the ring count follows from it rather than
    /// the other way round.
    fn faces(self) -> usize {
        match self {
            Self::Blade => 1,
            Self::Fibre => 3,
        }
    }
}

/// How wide a strand is at `t` along it, as a fraction of its root width.
///
/// **The one definition**, read by the mesh builder and by the splat, because
/// the relief and the geometry have to draw the same blade: a width profile
/// that differed between them would put the texture's blade at one silhouette
/// and the geometry's at another.
///
/// At a midpoint of zero this is `1 - taper * t`, which is the plain taper a
/// layer described before a midpoint existed. Above zero the blade narrows
/// towards the root at the same rate it narrows towards the tip, so the widest
/// part sits where the midpoint says — which is what gives the bright end of a
/// root-to-tip colour ramp any area at all. A fully tapered blade at midpoint
/// zero is a triangle whose bright end is one texel wide.
///
/// Never negative: a taper steep enough to run the width past zero ends the
/// blade in a point rather than turning it inside out.
#[must_use]
pub fn width_at(t: f32, taper: f32, midpoint: f32) -> f32 {
    let taper = taper.clamp(0.0, 1.0);
    let midpoint = midpoint.clamp(0.0, 1.0);
    (1.0 - taper * (t - midpoint).abs()).max(0.0)
}

/// The geometry of a strand layer, in the attributes a renderer binds.
///
/// Parallel vectors of arrays rather than a vertex struct or an engine mesh:
/// this crate has no renderer, and every consumer of these is going to hand
/// them to a different one under a different name. Every vector but
/// [`Self::indices`] is the same length.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrandMesh {
    /// Vertex positions, in the surface's own coordinates.
    pub positions: Vec<[f32; 3]>,
    /// Unit shading normals.
    pub normals: Vec<[f32; 3]>,
    /// `u` across the strand, `v` from zero at the root to one at the tip.
    pub uvs: Vec<[f32; 2]>,
    /// Linear RGBA: the strand's own root-to-tip gradient with its root
    /// occlusion already multiplied in, and an alpha of one.
    ///
    /// Opaque on purpose. The plan puts cutout cards in phase 4 and blending
    /// out of scope altogether, so alpha is here to fill the slot a renderer's
    /// colour attribute has rather than to say anything.
    pub colors: Vec<[f32; 4]>,
    /// What a wind stage needs and nothing else does: the strand's own phase,
    /// how far along it this vertex is, and how stiff it is.
    ///
    /// Unused until phase 4's vertex stage exists, and written now because the
    /// alternative is rebuilding every strand mesh in the workspace on the day
    /// it does. Three floats over a lawn is under a megabyte.
    pub wind: Vec<[f32; 3]>,
    /// Three vertex indices per triangle.
    pub indices: Vec<u32>,
}

impl StrandMesh {
    /// How many vertices it holds.
    pub fn vertices(&self) -> usize {
        self.positions.len()
    }

    /// How many triangles it holds.
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Whether nothing was built at all.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Build the geometry of every strand in `placed`.
///
/// One mesh rather than one per strand, because a strand is six triangles and a
/// draw call is not. Chunking — which strands belong in which mesh — is the
/// caller's, and [`PlacedStrand::repeat`] is what it groups by.
///
/// A strand of no length or no width is built all the same, as a curve of no
/// extent. The scatter's density threshold is what turns a strand *off*, and it
/// says so at length; a mesh whose vertex count depended on a field's value
/// would make every count here a thing a caller had to measure rather than
/// compute.
///
/// ```
/// use ashlar_strands::{PlacedStrand, Strand, StrandProfile, StrandShape, mesh};
///
/// let strand = Strand {
///     root: [0.5, 0.5], rank: 0.0, phase: 0.0, length: 0.1, width: 0.004,
///     direction: [1.0, 0.0], lean: 0.2, bend: 0.3,
///     root_color: [0.1, 0.2, 0.0], tip_color: [0.4, 0.5, 0.1], roughness: 0.9,
///     ..Default::default()
/// };
/// let placed = PlacedStrand {
///     strand,
///     position: [0.0; 3],
///     normal: [0.0, 1.0, 0.0],
///     tangent: [1.0, 0.0, 0.0],
///     bitangent: [0.0, 0.0, 1.0],
///     repeat: [0, 0],
/// };
/// let shape = StrandShape {
///     profile: StrandProfile::Blade,
///     segments: 3,
///     ..Default::default()
/// };
///
/// let built = mesh(&[placed], shape);
/// assert_eq!(built.vertices(), shape.vertices_per_strand());
/// assert_eq!(built.triangles(), shape.triangles_per_strand());
/// ```
#[must_use]
pub fn mesh(placed: &[PlacedStrand], shape: StrandShape) -> StrandMesh {
    let segments = shape.segments();
    let ring = shape.profile.ring();
    let vertices = placed.len() * shape.vertices_per_strand();
    let mut built = StrandMesh {
        positions: Vec::with_capacity(vertices),
        normals: Vec::with_capacity(vertices),
        uvs: Vec::with_capacity(vertices),
        colors: Vec::with_capacity(vertices),
        wind: Vec::with_capacity(vertices),
        indices: Vec::with_capacity(placed.len() * shape.triangles_per_strand() * 3),
    };
    for strand in placed {
        let base = u32::try_from(built.positions.len()).unwrap_or(u32::MAX);
        let stride = u32::try_from(ring).unwrap_or(1);
        Curve::of(strand).emit(strand, shape, segments, &mut built);
        for segment in 0..segments {
            let near = base + segment * stride;
            for face in 0..shape.profile.faces() {
                push_face(&mut built.indices, near, near + stride, face, ring);
            }
        }
    }
    built
}

/// The two triangles of one face of one segment.
///
/// Wound so the face points away from the strand's axis: outwards from a tube,
/// and, on a ribbon, towards whichever side a leaning blade tips its upper
/// surface to. The material draws these with backface culling off, so the
/// winding decides the shadow and the prepass rather than whether the blade is
/// visible at all.
fn push_face(indices: &mut Vec<u32>, near: u32, far: u32, face: usize, ring: usize) {
    let here = u32::try_from(face).unwrap_or(0);
    let next = u32::try_from((face + 1) % ring).unwrap_or(0);
    if ring == 2 {
        // A ribbon has no inside for its face to point away from, so the quad
        // is wound the other way round from a tube's.
        indices.extend_from_slice(&[
            near + here,
            far + here,
            near + next,
            near + next,
            far + here,
            far + next,
        ]);
        return;
    }
    indices.extend_from_slice(&[
        near + here,
        near + next,
        far + here,
        near + next,
        far + next,
        far + here,
    ]);
}

/// The three control points of one strand, and the frame its rings are built
/// in.
struct Curve {
    root: Vec3,
    /// The unit direction the curve leaves the root in.
    from_root: Vec3,
    /// The unit direction it arrives at the tip in.
    to_tip: Vec3,
    /// Perpendicular to the whole curve, and so to the lean plane: the axis a
    /// width is measured along, before [`Self::facing`] turns it.
    across: Vec3,
    length: f32,
    /// How far the ribbon is turned about its own curve, in radians.
    facing: f32,
}

impl Curve {
    /// The curve one planted strand describes.
    fn of(placed: &PlacedStrand) -> Self {
        let strand = &placed.strand;
        let up = Vec3::from_array(placed.normal);
        let tangent = Vec3::from_array(placed.tangent);
        let bitangent = Vec3::from_array(placed.bitangent);
        // The root sinks along the surface normal, which is how a pile gets
        // short, half-buried blades without shortening the distribution. It is
        // applied to the root alone: the curve keeps its whole length, and what
        // changes is where it starts.
        let root = Vec3::from_array(placed.position) - up * strand.height_offset.max(0.0);
        // The tip's pull towards the clump centre, out of UV and onto the
        // surface. A clump is a *curve* operator rather than a position: the
        // root stays where the lattice put it and the far end leans in, which
        // is what makes a tuft read as one thing rather than as a huddle.
        let gather = tangent * strand.clump_pull[0] + bitangent * strand.clump_pull[1];
        // The strand's own direction, out of UV and onto the surface. The frame
        // is orthonormal and the direction is unit or zero, so this is unit or
        // zero too, and no normalisation is needed to find out which.
        let forward = tangent * strand.direction[0] + bitangent * strand.direction[1];
        if forward.length_squared() < 0.5 {
            // No direction is no plane, so there is nothing for a lean or a
            // bend to tilt *into*: the strand stands up, and the width runs
            // along `+u` so that two upright strands still agree on which way
            // they face.
            return Self {
                root,
                from_root: up,
                // An upright strand still leans towards its clump, which is the
                // only thing that gathers a tuft of upright fibres.
                to_tip: (up + gather / strand.length.max(f32::MIN_POSITIVE)).normalize_or(up),
                across: tangent,
                length: strand.length,
                facing: strand.facing,
            };
        }
        let lean = strand.lean * std::f32::consts::FRAC_PI_2;
        let droop = lean + strand.bend * std::f32::consts::FRAC_PI_2;
        let axis = |angle: f32| {
            let (sine, cosine) = angle.sin_cos();
            up * cosine + forward * sine
        };
        Self {
            root,
            from_root: axis(lean),
            // The droop, turned towards the clump centre. The pull is a UV
            // displacement of the *tip*, so it is divided by the length to
            // become a direction the second Bézier tangent can carry.
            to_tip: (axis(droop) + gather / strand.length.max(f32::MIN_POSITIVE))
                .normalize_or(axis(droop)),
            // `up × forward` is perpendicular to both, and both Bézier tangents
            // are combinations of them, so it is perpendicular to every point
            // of the curve. One cross product for the whole strand.
            across: up.cross(forward).normalize_or(tangent),
            length: strand.length,
            facing: strand.facing,
        }
    }

    /// The tip, which is the curve's last control point.
    fn tip(&self) -> Vec3 {
        self.root + (self.from_root + self.to_tip) * (self.length * 0.5)
    }

    /// Where the curve is at `t`, and which way it is going.
    ///
    /// The control points are the root, the root plus half a length along
    /// [`Self::from_root`], and that plus half a length along [`Self::to_tip`],
    /// so the tangent is the straight interpolation of the two directions and
    /// the tip is [`Self::tip`] exactly.
    fn at(&self, t: f32) -> (Vec3, Vec3) {
        let half = self.length * 0.5;
        let first = self.root + self.from_root * half;
        let second = first + self.to_tip * half;
        let inverse = 1.0 - t;
        let point =
            self.root * (inverse * inverse) + first * (2.0 * inverse * t) + second * (t * t);
        let along = (self.from_root * inverse + self.to_tip * t).normalize_or(self.from_root);
        (point, along)
    }

    /// Every ring of one strand, appended to `built`.
    fn emit(
        &self,
        placed: &PlacedStrand,
        shape: StrandShape,
        segments: u32,
        built: &mut StrandMesh,
    ) {
        let strand = &placed.strand;
        let taper = shape.taper.clamp(0.0, 1.0);
        let occlusion = shape.root_occlusion.clamp(0.0, 1.0);
        // A strand already lying over has less upright cantilever left to bend,
        // so it is the stiff one. Phase 4's vertex stage is what reads it; it is
        // written here because a per-vertex fact belongs to the geometry.
        let stiffness = 1.0 - strand.lean.clamp(0.0, 1.0);
        for ring in 0..=segments {
            let v = ratio(ring, segments);
            let (centre, along) = self.at(v);
            let half = strand.width * 0.5 * width_at(v, taper, shape.midpoint);
            // Linear towards the tip, which is where the gradient a layer
            // authors is measured: the root colour is the colour at `v = 0`.
            let lit = 1.0 - occlusion * (1.0 - v);
            let color = [
                lerp(strand.root_color[0], strand.tip_color[0], v) * lit,
                lerp(strand.root_color[1], strand.tip_color[1], v) * lit,
                lerp(strand.root_color[2], strand.tip_color[2], v) * lit,
                1.0,
            ];
            let face = along.cross(self.across).normalize_or(self.across);
            // The ribbon turned about its own curve. `across` and `face` are
            // both perpendicular to `along` and to each other, so a rotation
            // about the tangent is the plain two-dimensional one in the plane
            // they span — no Rodrigues term survives. Turning them per ring
            // rather than once per strand is what keeps the twist honest on a
            // curve whose tangent moves.
            let (turn, straight) = self.facing.sin_cos();
            let across = self.across * straight + face * turn;
            let face = face * straight - self.across * turn;
            match shape.profile {
                StrandProfile::Blade => Self::blade(centre, across, face, half, v, built),
                StrandProfile::Fibre => Self::fibre(centre, across, face, half, v, built),
            }
            for _ in 0..shape.profile.ring() {
                built.colors.push(color);
                built.wind.push([strand.phase, v, stiffness]);
            }
        }
    }

    /// One ring of a ribbon: the two edges, with the normals splayed outwards.
    fn blade(centre: Vec3, across: Vec3, face: Vec3, half: f32, v: f32, built: &mut StrandMesh) {
        let (sine, cosine) = BLADE_CAMBER.sin_cos();
        for side in [-1.0_f32, 1.0] {
            built
                .positions
                .push((centre + across * (side * half)).to_array());
            built.normals.push(
                (face * cosine + across * (side * sine))
                    .normalize_or(face)
                    .to_array(),
            );
            // The side is the edge: `-1` is `u = 0` and `+1` is `u = 1`.
            built.uvs.push([side.mul_add(0.5, 0.5), v]);
        }
    }

    /// One ring of a three-sided tube.
    fn fibre(centre: Vec3, across: Vec3, face: Vec3, half: f32, v: f32, built: &mut StrandMesh) {
        for side in 0..3_u32 {
            let angle = ratio(side, 3) * std::f32::consts::TAU;
            let (sine, cosine) = angle.sin_cos();
            let radial = (across * cosine + face * sine).normalize_or(across);
            built.positions.push((centre + radial * half).to_array());
            built.normals.push(radial.to_array());
            // The ring shares its vertices rather than duplicating one at a
            // seam, so `u` wraps back to zero on the last side. A tube three
            // hairs wide carries no texture, and the honest seam would cost a
            // fourth vertex per ring on every fibre ever built.
            built.uvs.push([ratio(side, 3), v]);
        }
    }
}

/// `part / whole`, over counts small enough that `f32` holds both exactly.
///
/// Every caller here is a segment index against [`MAX_SEGMENTS`] or a side
/// index against three, so the conversion is a widening rather than a cast.
fn ratio(part: u32, whole: u32) -> f32 {
    let float = |value: u32| f32::from(u16::try_from(value).unwrap_or(u16::MAX));
    float(part) / float(whole.max(1))
}

/// Between two numbers, by `t`.
fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// Where a strand's tip ends up, as a plain function of what the strand carries.
///
/// The closed form the curve is built to meet: the root plus half a length
/// along each of the two Bézier tangents. A test that samples the curve at
/// `t = 1` and compares against this is checking the sampler against the
/// definition rather than against itself, and a caller placing something *at*
/// a tip — a seed head, a flower — needs the point and not the curve.
#[must_use]
pub fn tip_of(placed: &PlacedStrand) -> [f32; 3] {
    Curve::of(placed).tip().to_array()
}
