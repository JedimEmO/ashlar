//! A strand set, and everything a renderer does with one.
//!
//! A **strand** is one blade of grass, one hair of fur, one shoot of moss: a
//! root, a curve, a width and two colours. A [`StrandSet`] is every strand of
//! one layer over one repeat of a material, in rank order. This crate holds the
//! set, the [file](mod@file) it goes on disk in, and the three things that read
//! one: [`place`] plants it on a mesh, [`mesh`] turns the planted roots into
//! triangles, and [`cards`] draws it side on into the atlas an impostor wears.
//!
//! # Why this is a crate of its own
//!
//! Because of what is *not* here. A set is scattered from a material graph —
//! that is `ashlar_material::strands::scatter`, and it needs the graph engine:
//! a lowering, an interpreter, rasterised planes. Everything after the scatter
//! needs none of it. So the two halves are two crates, and this is the half a
//! game links: `glam`, `serde` and `thiserror`, no graph, no Bevy, no IO.
//!
//! ADR 0004 said the other thing — "generating a set is milliseconds, so there
//! is no strand file format and no serialized `StrandSet`" — and it was right
//! about the milliseconds and wrong about what followed from them. The cost
//! that matters is not the scatter's time, it is the *dependency*: a game that
//! had to scatter its own lawn would compile the whole graph engine to do it.
//! [`file`](mod@file) is what removes that, and the amendment of 2026-09-20 says so.
//!
//! # The shape of the answer
//!
//! ```
//! use ashlar_strands::{PlacedStrand, Strand, StrandSet, StrandShape, mesh, place};
//!
//! // One strand, at the middle of the repeat. A set is normally scattered;
//! // this is the by-hand form, which is what a test and a file reader build.
//! let set = StrandSet::new(
//!     "blades".to_owned(),
//!     vec![Strand {
//!         root: [0.5, 0.5],
//!         length: 0.1,
//!         width: 0.004,
//!         direction: [1.0, 0.0],
//!         lean: 0.2,
//!         ..Default::default()
//!     }],
//!     [16, 16],
//!     StrandShape::default(),
//! );
//!
//! // One square metre of ground, mapped to exactly one repeat.
//! let placed = place(
//!     &set,
//!     &ashlar_strands::SurfaceTriangles {
//!         positions: &[[0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
//!         normals: &[[0.0, 1.0, 0.0]; 4],
//!         uvs: &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
//!         indices: &[0, 1, 2, 0, 2, 3],
//!     },
//!     1.0,
//! );
//! let built = mesh(&placed, set.shape());
//! assert_eq!(built.triangles(), set.shape().triangles_per_strand());
//! ```
mod cards;
pub mod file;
mod geometry;
mod placement;

use serde::{Deserialize, Serialize};

pub use cards::{CardAtlas, CardRequest, cards};
pub use geometry::{BLADE_CAMBER, StrandMesh, StrandShape, mesh, tip_of, width_at};
pub use placement::{PlacedStrand, SurfaceTriangles, place};

/// What shape a strand is built as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum StrandProfile {
    /// A tapered ribbon, flat across its width: grass, a leaf, a frond.
    #[default]
    Blade,
    /// A three-sided tube: fur, a bristle, a straw of thatch, carpet pile.
    Fibre,
}

/// The most segments a strand may be built from.
///
/// A strand is a curve sampled into a ribbon or a tube, and the segment count
/// is what that costs: a repeat of 65 536 strands at three segments is already
/// half a million triangles. `ashlar-material` re-exports it and validates a
/// layer against it, because a count a graph declares is a count a validation
/// can refuse and a layer that asks for a thousand segments is an author who
/// meant something else. It lives here because [`StrandShape::segments`] is
/// what finally holds a count to it, and a file read back off disk has to be
/// held to the same bound as one a graph declared.
pub const MAX_SEGMENTS: u32 = 16;

/// One strand of a set.
///
/// Everything a mesh builder needs and nothing it can derive: the root is in UV
/// over one repeat, the lengths are in metres, and the colours are linear. The
/// layer's own constants — the profile, the segment count, the taper and the
/// root occlusion — are the same for every strand and are carried by the
/// [`StrandSet`] instead.
/// `Default` is every field at rest: a strand of no length or width at the
/// origin, upright, unclumped and unturned. It is not a strand a scatter ever
/// answers — a scatter fills every field — and it is here so that a caller
/// building one by hand, which is a test or a tool, names the fields it cares
/// about and leaves the rest alone rather than being broken by a field this
/// type grows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Strand {
    /// Where the strand is rooted, in UV over one repeat.
    pub root: [f32; 2],
    /// Where the strand falls in the level-of-detail order, in `0..=1`.
    ///
    /// A hash of the cell and nothing else, so a cut at a rank keeps the same
    /// strands whatever else changed. [`StrandSet::prefix`] is the cut.
    pub rank: f32,
    /// A free hash in `0..=1`, for whatever a renderer wants one for. Wind is
    /// what it is there for: a blade that sways in step with its neighbour
    /// reads as a flag rather than as grass.
    pub phase: f32,
    /// How long the strand is, in metres.
    pub length: f32,
    /// How wide it is at the root, in metres.
    pub width: f32,
    /// Which way it leans, in the UV plane: unit length, or zero for upright.
    pub direction: [f32; 2],
    /// How far the root tilts away from the surface normal, in `0..=1` of a
    /// quarter turn.
    pub lean: f32,
    /// How much the strand droops between root and tip.
    pub bend: f32,
    /// Linear colour at the root.
    pub root_color: [f32; 3],
    /// Linear colour at the tip.
    pub tip_color: [f32; 3],
    /// Perceptual roughness.
    pub roughness: f32,
    /// How far the strand is turned about its own curve, in radians.
    ///
    /// Zero presents the ribbon's face along the lean plane's normal, which is
    /// what every strand did before a layer could vary the facing. A quarter
    /// turn is edge-on.
    pub facing: f32,
    /// How far the root sits *below* the surface, in metres.
    pub height_offset: f32,
    /// Which clump the strand belongs to, as a hash in `0..=1`, or zero for a
    /// layer that declares no clump lattice.
    ///
    /// The identity every member of one tuft shares. It is what a per-clump
    /// tint reads and what a renderer would group by.
    pub clump_id: f32,
    /// How far out in its clump the strand stands, in `0..=1` from the centre
    /// to the edge of the clump's own cell, and zero where there is no clump.
    ///
    /// What a curve-shaped clump operator needs and a position offset does not:
    /// Blender's Clump Shape says how the pull varies along the strand, and
    /// this is the other axis — how much of it a strand takes at all. A blade
    /// at the centre of a tuft is already where the tuft is going; one at its
    /// edge is the one that has to lean in.
    pub clump_distance: f32,
    /// The vector from the root to its clump's centre, in UV over one repeat,
    /// and zero where there is no clump.
    ///
    /// A *tip* pull rather than a root offset, which is the whole difference
    /// between a clump and a huddle: the geometry leans the strand's far end
    /// towards this point and leaves the root where the lattice put it.
    pub clump_pull: [f32; 2],
}

/// How many `f32` one [`Strand`] is made of.
///
/// Written out rather than derived, because [`file`](mod@file) stores a set field by
/// field and this is the number of fields it walks. A strand that grew a
/// sixteenth field would break the file's version, and the test that holds
/// `size_of::<Strand>()` against this is what says so at the moment it happens
/// rather than at the moment a lawn comes back wrong.
pub const FLOATS_PER_STRAND: usize = 23;

/// Every strand of one layer over one repeat, in rank order.
///
/// Sorted ascending by [`Strand::rank`], which is what makes a level of detail
/// a [`Self::prefix`] of the slice: the survivors of a cut are a subset of the
/// survivors of a looser one, so nothing reshuffles when a chunk changes level.
/// [`Self::new`] is what establishes that, so a set from a scatter, a set read
/// off disk and a set built by hand all carry the same guarantee.
#[derive(Clone, Debug, PartialEq)]
pub struct StrandSet {
    layer: String,
    strands: Vec<Strand>,
    count: [u32; 2],
    shape: StrandShape,
}

impl StrandSet {
    /// A set from the strands it holds, sorted into rank order.
    ///
    /// The sort is **stable**, and that is load-bearing rather than incidental:
    /// a scatter hands over a vector built in row-major cell order, so a rank
    /// that two strands happen to share is broken by the cell they came from
    /// and the set is one answer rather than whichever order a sort happened to
    /// land on. A set that is already in rank order — one read back from a
    /// [`file`](mod@file) — passes through unchanged, because a stable sort over a sorted
    /// slice moves nothing.
    ///
    /// `count` is the lattice the roots were scattered on and is carried rather
    /// than derived: the roots have been moved by their clumps by the time they
    /// arrive here, so there is nothing in them to read a period back out of.
    #[must_use]
    pub fn new(
        layer: String,
        mut strands: Vec<Strand>,
        count: [u32; 2],
        shape: StrandShape,
    ) -> Self {
        strands.sort_by(|left, right| left.rank.total_cmp(&right.rank));
        Self {
            layer,
            strands,
            count,
            shape,
        }
    }

    /// The name of the layer this came from.
    pub fn layer(&self) -> &str {
        &self.layer
    }

    /// Every strand, in rank order.
    pub fn strands(&self) -> &[Strand] {
        &self.strands
    }

    /// The strands whose rank is below `keep`, which is the level-of-detail
    /// cut: `0.25` keeps about a quarter of them, spread evenly over the
    /// repeat, and they are the same quarter every time.
    ///
    /// A prefix of the slice rather than a filter over it, because the set is
    /// sorted by rank. `keep` at or above one is the whole set, and at or below
    /// zero is nothing.
    pub fn prefix(&self, keep: f32) -> &[Strand] {
        let end = self.strands.partition_point(|strand| strand.rank < keep);
        &self.strands[..end]
    }

    /// How many strands there are, after the density threshold took its share.
    pub fn len(&self) -> usize {
        self.strands.len()
    }

    /// Whether the density threshold kept none at all.
    pub fn is_empty(&self) -> bool {
        self.strands.is_empty()
    }

    /// The lattice the roots were scattered on, per axis.
    pub fn count(&self) -> [u32; 2] {
        self.count
    }

    /// The shape constants this set's layer declared.
    pub fn shape(&self) -> StrandShape {
        self.shape
    }

    /// Which shape the strands are built as.
    pub fn profile(&self) -> StrandProfile {
        self.shape.profile
    }

    /// How many segments the curve is built from.
    pub fn segments(&self) -> u32 {
        self.shape.segments
    }

    /// How much of the root width the tip gives up.
    pub fn taper(&self) -> f32 {
        self.shape.taper
    }

    /// How much the vertex colour darkens at the root.
    pub fn root_occlusion(&self) -> f32 {
        self.shape.root_occlusion
    }

    /// Where along a strand it is widest, in `0..=1`.
    pub fn midpoint(&self) -> f32 {
        self.shape.midpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_is_sorted_by_rank_however_its_strands_arrived() {
        // The invariant `prefix` is a prefix *because of*: a caller handing
        // over an unsorted vector — a test, a tool, a file whose bytes were
        // edited — still gets a set a level of detail can cut.
        let strand = |rank: f32| Strand {
            rank,
            ..Default::default()
        };
        let set = StrandSet::new(
            "blades".to_owned(),
            vec![strand(0.7), strand(0.1), strand(0.4)],
            [4, 4],
            StrandShape::default(),
        );
        let ranks: Vec<f32> = set.strands().iter().map(|strand| strand.rank).collect();
        assert_eq!(ranks, [0.1, 0.4, 0.7]);
        assert_eq!(set.prefix(0.5).len(), 2);
        assert_eq!(set.prefix(0.0).len(), 0);
        assert_eq!(set.prefix(1.0).len(), 3);
    }

    #[test]
    fn a_strand_is_the_floats_the_file_format_walks() {
        // `file` stores a set field by field and trusts this number. A strand
        // that grew a field would otherwise write a short record and read a
        // lawn back shifted by one.
        assert_eq!(
            size_of::<Strand>(),
            FLOATS_PER_STRAND * size_of::<f32>(),
            "a strand is no longer {FLOATS_PER_STRAND} floats, so the file format's version has \
             to move with it"
        );
    }
}
