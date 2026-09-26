//! Growing a strand layer on a meshed surface, from a set already in hand.
//!
//! An [`ashlar::MaterialDefinition`] may carry [`ashlar::StrandSettings`]
//! beside its surface: which strand layers to grow, how densely, how far out,
//! and — since 2026-09-20 — which baked set file to grow them from. This module
//! is what turns that into geometry. It reads the sets, plants their roots on
//! the surface's triangles, builds the ribbons or tubes, and hands back
//! [`Mesh`]es and the [`StandardMaterial`] that draws them.
//!
//! **Where the sets come from is the feature split.** [`StrandSets::read`] is
//! the game half: a file, opened, validated and turned into geometry with no
//! graph engine anywhere in the tree. `scatter_sets`, behind
//! `strand-scatter`, is the tool half that *wrote* that file. Everything below
//! the sets — placement, the mesh builder, the levels, the cards, the wind — is
//! the same code either way, and that is the point: what a content step draws
//! in the preview is what a game draws off disk.
//!
//! Five things are worth knowing before leaning on it.
//!
//! **A chunk is a UV repeat.** [`create_strands`] answers one mesh per repeat
//! of the material the surface covers rather than one mesh for the whole wall,
//! because a repeat is already a compact patch of surface and already the unit
//! the scatter is defined over. That is what gives frustum culling something to
//! cull and what a [`VisibilityRange`] attaches to. A chunk carries its own
//! [`origin`](StrandChunk::origin) and its mesh is written relative to it, so
//! the distance a level of detail is measured at is the distance to that patch
//! of lawn.
//!
//! **A level of detail is a rank cut, not a second scatter.** [`strand_levels`]
//! answers one [`StrandLevel`] per distance the definition named, each keeping
//! a *prefix* of the set — so the survivors of a tighter cut are a subset of the
//! survivors of a looser one and nothing reshuffles at a switch — widened by
//! `1/sqrt(keep)` to put back the coverage and built from one segment fewer.
//! Past the last distance there is the card level where a definition names a
//! [`card_metres`](ashlar::StrandSettings::card_metres) and nothing where it
//! does not, and past *that* what the camera sees is the same strands splatted
//! into the material by a `StrandRelief` node. [`cards`](crate::cards) is the
//! level in between and says what a card is.
//!
//! **The set is shared and the geometry is not.** A set is the same
//! sixty-five thousand strands for every wall wearing the material — one file,
//! read once — and where those strands *land* depends on the mesh under them,
//! so placement and meshing run per surface. The card atlas is shared the same
//! way, through [`CardCache`](crate::cards::CardCache).
//!
//! **Nothing here is asynchronous.** [`create_strands`] places and builds on
//! the calling thread, which is what a preview and a content step want. A game
//! growing a lawn while it is being played wants the same calls in a task and
//! only the assets on the main thread; every type this module answers with is
//! `Send`. The one thing that cannot move off it is a layer's card atlas,
//! because handing two images over is `Assets<Image>` and that is the ECS's —
//! which is why [`StrandContext`] carries one.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use ashlar_strands::{PlacedStrand, StrandSet, StrandShape, SurfaceTriangles, file, mesh, place};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::VisibilityRange,
    mesh::{MeshVertexAttribute, VertexFormat},
    prelude::*,
};

#[cfg(feature = "strand-scatter")]
use crate::runtime_bake::{BakeCache, ParamBits, graph_of};
#[cfg(feature = "strand-scatter")]
use ashlar_material::{MaterialGraphLibrary, ParamValue};
#[cfg(feature = "strand-scatter")]
use std::num::NonZeroUsize;

/// Texels per repeat for any plane a strand field reads.
///
/// [`ashlar_material::strands::FIELD_RESOLUTION`] itself, re-exported under the
/// name this crate has always called it, and **not** a second number that
/// happens to agree. A `StrandRelief` splats its layer from a scatter at that
/// constant, and a scatter here has to run at the same one or the blades in the
/// texture lean differently from the blades standing on it.
///
/// It is part of what decides the answer and so part of [`StrandKey`]. A caller
/// may put another number in [`ScatterContext::field_resolution`], and one that
/// does takes that disagreement on knowingly.
#[cfg(feature = "strand-scatter")]
pub const STRAND_FIELDS: u32 = ashlar_material::strands::FIELD_RESOLUTION;

/// The wind attribute a strand mesh carries: the strand's own phase, how far
/// along it the vertex is, and how stiff it is.
///
/// Written by every strand mesh and read by the `wind` materials' vertex
/// stage. A strand drawn with a plain [`StandardMaterial`] carries it and
/// ignores it, because an attribute nothing binds costs a vertex buffer and no
/// pipeline: Bevy specialises a pipeline from the attributes the *shader* asks
/// for, so one the shader does not mention is carried and ignored.
///
/// The id is a constant of this crate rather than one of Bevy's, which is what
/// a custom attribute is; it is far above the small numbers the built-in
/// attributes use so that the two cannot collide.
pub const ATTRIBUTE_STRAND_WIND: MeshVertexAttribute =
    MeshVertexAttribute::new("StrandWind", 0x0057_494e_4400, VertexFormat::Float32x3);

/// What makes two scatters the same scatter: the graph, the layer, the
/// resolution its fields are read at, and every parameter value.
///
/// [`BakeKey`](crate::runtime_bake::BakeKey)'s policy applied to a strand layer,
/// and argued for at length there: the exact values rather than a digest of
/// them, because a collision would not be a slow frame but one lawn wearing
/// another's blades.
///
/// The layer name is in the key because one graph may declare several — blades
/// and seed heads over one field — and they are different sets. The field
/// resolution is in it because a buffered filter upstream of a field is
/// rasterised at that resolution, so two requests at two resolutions are two
/// answers. The parameters are the *folded* ones: a strand set is geometry, so
/// even a `Live` parameter is bound at a value and rebuilt rather than driven
/// by a uniform.
///
/// Like every key in this cache it names a graph rather than describing one, so
/// a library reloaded in place with the same keys and different graphs must
/// [`clear`](BakeCache::clear) the cache.
#[cfg(feature = "strand-scatter")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StrandKey {
    /// Content key of the graph.
    graph: String,
    /// The layer of that graph, by the name it declares it under.
    layer: String,
    /// Texels per repeat for the planes its fields read.
    field_resolution: u32,
    /// Every bound parameter, in the [`BTreeMap`] order the surface carries
    /// them in, so the same map is always the same key.
    params: Vec<(String, ParamBits)>,
}

#[cfg(feature = "strand-scatter")]
impl StrandKey {
    /// The key of one layer of one surface's graph.
    pub fn new(
        graph: &str,
        layer: &str,
        field_resolution: u32,
        params: &BTreeMap<String, ParamValue>,
    ) -> Self {
        Self {
            graph: graph.to_owned(),
            layer: layer.to_owned(),
            field_resolution,
            params: params
                .iter()
                .map(|(name, value)| (name.clone(), ParamBits::from(value)))
                .collect(),
        }
    }

    /// The graph this key is for.
    pub fn graph(&self) -> &str {
        &self.graph
    }

    /// The layer of it.
    pub fn layer(&self) -> &str {
        &self.layer
    }

    /// Texels per repeat of the planes its fields read.
    pub fn field_resolution(&self) -> u32 {
        self.field_resolution
    }

    /// This key as the one string a card atlas is cached under.
    ///
    /// Everything but the layer, which [`CardKey`](crate::cards::CardKey)
    /// carries beside it. Written out field by field rather than taken from
    /// [`Debug`], because a cache key that changed when a derive's formatting
    /// changed would silently redraw every atlas in the scene; and exactly,
    /// rather than as a digest, for the reason this type gives above.
    pub fn origin(&self) -> String {
        use std::fmt::Write as _;

        let mut origin = format!("graph:{}@{}", self.graph, self.field_resolution);
        for (name, value) in &self.params {
            // The write cannot fail — the target is a `String` — and the
            // `Result` is discarded rather than unwrapped for that reason.
            let _ = write!(origin, ";{name}={value:?}");
        }
        origin
    }
}

/// What [`create_strands`] needs beyond the definition, the surface and the
/// sets.
///
/// A borrow bundle in the shape `runtime_bake::BakeContext`
/// has, and for the same reason: a system that grows strands already holds both
/// of these as parameters. Note what is *not* in it — a graph library — which
/// is the whole of the 2026-09-20 split: growing a lawn takes a set and a mesh,
/// and where the set came from is somebody else's question.
pub struct StrandContext<'a> {
    /// Where a card atlas's images are put.
    ///
    /// The one thing here that is the ECS's, and the reason the geometry half
    /// of this module is still plain data: a card level binds a *texture*, so
    /// the layer that grows one has to hand two images over. A definition that
    /// asks for no cards never touches this.
    pub images: &'a mut Assets<Image>,
    /// Where a card atlas is found and remembered, so that every wall wearing
    /// one material binds one picture rather than one each.
    pub cards: &'a mut crate::cards::CardCache,
}

impl<'a> StrandContext<'a> {
    /// The context a caller with an image collection and a card cache wants.
    pub fn new(images: &'a mut Assets<Image>, cards: &'a mut crate::cards::CardCache) -> Self {
        Self { images, cards }
    }
}

/// What `scatter_sets` needs to grow a set out of a graph: the tool half.
///
/// Separate from [`StrandContext`] rather than folded into it, and that
/// separation is the feature split made visible. A scatter is a graph, a cache
/// of graph answers and a thread budget; growing geometry is none of those. A
/// caller holding both passes both, and a game holds only the second.
#[cfg(feature = "strand-scatter")]
pub struct ScatterContext<'a> {
    /// The library the surface's graph is named in.
    pub graphs: &'a MaterialGraphLibrary,
    /// Where a scatter's set is found and remembered.
    pub cache: &'a mut BakeCache,
    /// How many threads the scatter divides its cell rows across. `None` is one
    /// per core, bounded by the work; a caller sharing the machine with a
    /// renderer says a number. The set is the same either way.
    pub threads: Option<NonZeroUsize>,
    /// Texels per repeat for the planes a field reads. [`STRAND_FIELDS`] is
    /// what this should be unless a caller has a reason.
    pub field_resolution: u32,
}

#[cfg(feature = "strand-scatter")]
impl<'a> ScatterContext<'a> {
    /// The context a caller with a graph library and a cache wants, at the
    /// default field resolution.
    pub fn new(graphs: &'a MaterialGraphLibrary, cache: &'a mut BakeCache) -> Self {
        Self {
            graphs,
            cache,
            threads: None,
            field_resolution: STRAND_FIELDS,
        }
    }

    /// Bound the threads the scatter divides its rows across.
    #[must_use]
    pub fn threads(mut self, threads: Option<NonZeroUsize>) -> Self {
        self.threads = threads;
        self
    }
}

/// Every layer of one definition's strand settings, with where they came from.
///
/// The value the two halves of this crate meet on. A game builds one with
/// [`Self::read`], out of the file its definition names; a tool builds one with
/// `scatter_sets`, out of the graph. What [`create_strands`] takes is this,
/// and it cannot tell the two apart.
///
/// [`Self::origin`] is what a card atlas is cached under, and it is the asset
/// key one way round and the graph and its bound values the other. It only has
/// to be *distinct*, and an asset path is not a graph key.
#[derive(Clone, Debug, PartialEq)]
pub struct StrandSets {
    origin: String,
    sets: Vec<StrandSet>,
}

impl StrandSets {
    /// The sets in one baked file, as the definition that names it grows them.
    ///
    /// `key` is the asset key the bytes were read from, which is what the
    /// atlases drawn from these sets are cached under and what an error names.
    ///
    /// # Errors
    ///
    /// If the bytes are not a baked strand set, are a version this build does
    /// not read, or name anything outside themselves.
    /// [`read_library`](crate::read_library)'s own strand preflight has already refused all three
    /// before a window exists, so a failure here is a file that changed under a
    /// running game.
    pub fn read(key: &str, bytes: &[u8]) -> Result<Self> {
        let sets = file::read(bytes).with_context(|| format!("reading strand set {key}"))?;
        Ok(Self {
            origin: key.to_owned(),
            sets,
        })
    }

    /// Sets a caller assembled, named by where they came from.
    #[must_use]
    pub fn of(origin: String, sets: Vec<StrandSet>) -> Self {
        Self { origin, sets }
    }

    /// Where these came from: an asset key, or a graph and its bound values.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Every set, in the order they were built or read.
    #[must_use]
    pub fn sets(&self) -> &[StrandSet] {
        &self.sets
    }

    /// The set of one layer, by the name the graph declared it under.
    #[must_use]
    pub fn get(&self, layer: &str) -> Option<&StrandSet> {
        self.sets.iter().find(|set| set.layer() == layer)
    }

    /// Which layers these hold, for an error that has to say what *is* here.
    fn declared(&self) -> String {
        if self.sets.is_empty() {
            return "none at all".to_owned();
        }
        self.sets
            .iter()
            .map(StrandSet::layer)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One repeat of one layer, as a mesh.
#[derive(Debug)]
pub struct StrandChunk {
    /// Which repeat of the material this is, as [`PlacedStrand::repeat`] names
    /// it. What a caller names an entity after, and what a spatial query would
    /// key on.
    pub repeat: [i32; 2],
    /// Where the chunk sits in the surface's own coordinates: the mean of its
    /// roots, which [`Self::mesh`] is written *relative to*.
    ///
    /// A chunk is the unit a level of detail switches at, and Bevy measures the
    /// distance to a [`VisibilityRange`] from the entity's own origin rather
    /// than from its bounds — deliberately, because two levels of one mesh have
    /// different bounds and a crossfade needs both at precisely the same point.
    /// So the origin has to be the chunk's, and the way to give an entity an
    /// origin is to move it into the transform. Every level of one chunk shares
    /// this point, which is what makes the fade between them exact.
    pub origin: [f32; 3],
    /// How many strands it holds, or, on a chunk of the card level, how many
    /// cards — one per tuft, each standing in for a clump of them.
    pub strands: usize,
    /// The geometry, ready for `Assets<Mesh>`.
    pub mesh: Mesh,
}

/// One level of detail of one layer: the chunks, and where it is drawn.
pub struct StrandLevelMesh {
    /// What this level keeps and how it compensates. [`StrandLevel`] says.
    pub level: StrandLevel,
    /// One chunk per repeat the surface reaches, in a deterministic order.
    ///
    /// The same repeats at every level: a level is a rank prefix, so it thins
    /// a chunk rather than removing one, and a chunk that fell empty is still
    /// here as an empty mesh so that the levels line up.
    pub chunks: Vec<StrandChunk>,
}

/// The card level of one layer: the quads, where they are drawn, and the atlas
/// they wear.
///
/// The level past the last [`StrandLevelMesh`], and the last thing drawn at
/// all: past [`range`](Self::range) there is no geometry and what the camera
/// sees is the relief. [`cards`](crate::cards) is what builds it and says what
/// a card is.
pub struct StrandCardMesh {
    /// Where the cards are drawn, as [`cards::band`](crate::cards::band) puts
    /// it: from the last level's own margin out to the definition's
    /// `card_metres`.
    pub range: VisibilityRange,
    /// One chunk per repeat, in the same order and about the same origins as
    /// the levels above it. [`StrandChunk::strands`] counts *cards* here, one
    /// per tuft.
    pub chunks: Vec<StrandChunk>,
    /// The picture they wear, shared by every surface with the same scatter.
    pub atlas: crate::cards::CardImages,
    /// The alpha the cutout is tested against, which is the one the mip chain
    /// preserved the coverage at.
    pub cutoff: f32,
    /// What one layer's cards are drawn and built at.
    pub settings: crate::cards::CardSettings,
}

/// One strand layer of one surface, by level and then by repeat.
pub struct StrandLayerMesh {
    /// The layer of the graph this came from.
    pub layer: String,
    /// One entry per level of detail, nearest first. Always at least one.
    pub levels: Vec<StrandLevelMesh>,
    /// The card level past the last of them, where the definition asks for one.
    pub cards: Option<StrandCardMesh>,
    /// How many strands were planted at full detail, over every chunk.
    pub strands: usize,
    /// How many triangles every level came to together, which is what the
    /// layer actually costs in memory.
    pub triangles: usize,
    /// The mean perceptual roughness of the strands that were planted.
    ///
    /// A [`StandardMaterial`] carries one roughness and a strand layer carries
    /// one per strand, so what [`strand_material`] binds is this. The per-strand
    /// value is not lost — it is a field of the set — but no material reads
    /// it per vertex, so a layer whose roughness field varies is drawn at its
    /// average rather than silently at the definition's.
    pub roughness: f32,
}

/// The triangles of a meshed surface, in the arrays placement takes.
///
/// The one conversion this crate owns and the reason placement could stay in
/// `ashlar-material`: positions narrow from `f64` to `f32`, and the UVs — which
/// `ashlar` measures in **metres** — are divided by the definition's own
/// `tile_metres` and shifted by its `uv_offset`, which is exactly the transform
/// [`create_material`](crate::create_material) puts on the material's own sampling. So a blade
/// stands where the texel under it says it should.
pub struct StrandSurface {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl StrandSurface {
    /// One meshed element, in repeats of the material a definition binds.
    // The renderer is the f64-to-f32 boundary, here as everywhere else.
    #[allow(clippy::cast_possible_truncation)]
    pub fn of(surface: &ashlar::TriangleMesh, definition: &ashlar::MaterialDefinition) -> Self {
        let scale = Vec2::from_array(definition.tile_metres).recip();
        let offset = Vec2::from_array(definition.uv_offset);
        Self {
            positions: surface
                .positions
                .iter()
                .map(|point| point.as_vec3().to_array())
                .collect(),
            normals: surface
                .normals
                .iter()
                .map(|normal| normal.as_vec3().to_array())
                .collect(),
            uvs: surface
                .uvs
                .iter()
                .map(|uv| (Vec2::new(uv[0] as f32, uv[1] as f32) * scale + offset).to_array())
                .collect(),
            indices: surface.indices.clone(),
        }
    }

    /// The borrowed view placement takes.
    pub fn triangles(&self) -> SurfaceTriangles<'_> {
        SurfaceTriangles {
            positions: &self.positions,
            normals: &self.normals,
            uvs: &self.uvs,
            indices: &self.indices,
        }
    }
}

/// Validate a definition's strand settings against the graph library.
///
/// The half of [`ashlar::StrandSettings`] that `ashlar` cannot check, and the
/// reason it is here: `ashlar::MaterialDefinition::check` can see that a
/// surface names *a* graph, and only a crate holding the graph library can see
/// whether that graph declares a layer called `blades`. So a misspelled layer
/// is a startup error naming the material and the layer, exactly as a
/// misspelled parameter is.
///
/// What it deliberately does not do is scatter one. A scatter is sixty-five
/// thousand field evaluations and the refusals left after this — a field that
/// reaches the frame, a field that does not tile — are ones the graph library's
/// own validation already made when it was read.
#[cfg(feature = "strand-scatter")]
pub fn preflight_strands(
    definition: &ashlar::MaterialDefinition,
    graphs: &MaterialGraphLibrary,
) -> Result<()> {
    let Some(settings) = &definition.strands else {
        return Ok(());
    };
    // `check_library` has already refused a strand setting on a surface that names no
    // graph, and it ran before this.
    let Some(key) = definition.surface.graph() else {
        return Ok(());
    };
    let graph = graph_of(key, graphs)?;
    for layer in &settings.layers {
        anyhow::ensure!(
            graph.strands.contains_key(layer),
            "material graph {key:?} declares no strand layer {layer:?}; it declares {}",
            declared(graph)
        );
    }
    Ok(())
}

/// The layers a graph does declare, for the error above.
#[cfg(feature = "strand-scatter")]
fn declared(graph: &ashlar_material::MaterialGraph) -> String {
    if graph.strands.is_empty() {
        return "none at all".into();
    }
    graph
        .strands
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What each level of detail keeps of the one before it.
///
/// A quarter, which is two blades in four: the rank cut is a *prefix*, so the
/// survivors are the same quarter every time and nothing reshuffles when a
/// chunk changes level. A gentler cut would not pay for the extra draw call,
/// and a harsher one leaves too little to widen back into.
pub const LEVEL_KEEP: f32 = 0.25;

/// How much of its length the last level of detail keeps.
///
/// The last band is where the geometry stops and the relief takes over, and
/// something has to happen at that line or the blades simply vanish. What
/// happens is that they are already short: at a little under half the pile
/// height they sit *inside* the relief the texture is drawing, so the entity
/// fading out subtracts almost nothing from the silhouette.
///
/// Baked into the mesh rather than scaled per frame, which is the cheap answer
/// and is honest about it: a continuous shrink across the band would scale
/// each vertex towards a root it does not carry (the `wind` module says why
/// that was not taken). So the shrink is one step at one distance, hidden
/// inside Bevy's own dithered crossfade.
pub const LAST_BAND_LENGTH: f32 = 0.45;

/// How much light a blade passes through itself.
///
/// A leaf is thin and translucent: lit from behind it glows, and a renderer
/// that treats it as opaque leaves every backlit blade on ambient alone. At a
/// grazing angle that is most of the blades in frame, which is where a lawn
/// starts reading as soot rather than as grass.
///
/// An eighth rather than the half a real leaf would take, because the vertex
/// colour already carries a root-to-tip gradient and the pile's own dark is in
/// the texture under it: a brighter transmitted term lifts every backlit blade
/// towards the same value and flattens both.
pub const BLADE_TRANSMISSION: f32 = 0.12;

/// How wide the crossfade between two levels is, as a fraction of the distance
/// it happens at.
///
/// A tenth either side, so a switch at eight metres fades from 7.2 to 8.8. Wide
/// enough that the dither has frames to work with at walking pace, narrow
/// enough that two levels are rarely both drawn.
///
/// Read by [`cards::band`](crate::cards::band) as well, because the card band
/// takes over at the last level's own margin and two margins that disagreed
/// would leave a gap with nothing in it or an overlap with two of everything.
pub(crate) const CROSSFADE: f32 = 0.1;

/// One level of detail of a strand layer: what it keeps, and where it is drawn.
///
/// The three compensations are what keep a cut from looking like a cut. Fewer
/// strands is the saving; wider blades put back the coverage the missing ones
/// took away — a quarter as many blades at twice the width covers the same
/// ground, which is why the factor is `1/sqrt(keep)` and not something tuned —
/// and fewer segments is the rest of the saving, taken where a curve is already
/// below a pixel.
// `VisibilityRange` is `Clone + PartialEq` and not `Debug`, so neither is this.
#[derive(Clone, PartialEq)]
pub struct StrandLevel {
    /// Which level this is, zero being full detail.
    pub level: usize,
    /// The rank below which a strand is kept, which is an *absolute* cut
    /// against [`Strand::rank`](ashlar_strands::Strand::rank) with
    /// the definition's own density already in it.
    pub keep: f32,
    /// How many segments a strand is built from here.
    pub segments: u32,
    /// What a strand's width is multiplied by.
    pub width_scale: f32,
    /// What a strand's length is multiplied by.
    pub length_scale: f32,
    /// Where this level is drawn, or `None` for a layer that declared no
    /// distances and is therefore always drawn.
    pub range: Option<VisibilityRange>,
}

impl StrandLevel {
    /// The shape a level is built at: the layer's own, with this level's
    /// segment count.
    pub fn shape(&self, shape: ashlar_strands::StrandShape) -> StrandShape {
        StrandShape {
            segments: self.segments,
            ..shape
        }
    }

    /// One placed strand as this level draws it: wider, shorter, and moved into
    /// the chunk's own frame.
    ///
    /// The scales are baked into the strand rather than into the mesh because a
    /// strand is where the width and the length *mean* something — a taper and
    /// a Bézier are both derived from them — so scaling the vertices afterwards
    /// would widen the tip of a fully tapered blade back off its point.
    fn scaled(&self, placed: &PlacedStrand, origin: [f32; 3]) -> PlacedStrand {
        let mut placed = *placed;
        placed.strand.width *= self.width_scale;
        placed.strand.length *= self.length_scale;
        for (axis, centre) in placed.position.iter_mut().zip(origin) {
            *axis -= centre;
        }
        placed
    }
}

/// The levels one definition's strand settings ask for, nearest first.
///
/// One level per distance in [`ashlar::StrandSettings::lod_metres`], and one
/// level with no range at all where it names none. Past the last distance there
/// is no entry *here*, whatever the definition says about cards:
/// [`cards::band`](crate::cards::band) is the level after these, and it starts
/// at the margin the last of them ends on.
pub fn strand_levels(
    settings: &ashlar::StrandSettings,
    shape: ashlar_strands::StrandShape,
) -> Vec<StrandLevel> {
    let density = settings.density.clamp(0.0, 1.0);
    if settings.lod_metres.is_empty() {
        return vec![StrandLevel {
            level: 0,
            keep: density,
            segments: shape.segments(),
            width_scale: 1.0,
            length_scale: 1.0,
            range: None,
        }];
    }
    let last = settings.lod_metres.len() - 1;
    let mut levels = Vec::with_capacity(settings.lod_metres.len());
    let mut start = 0.0_f32..0.0_f32;
    for (level, end) in settings.lod_metres.iter().enumerate() {
        let fraction = LEVEL_KEEP.powi(i32::try_from(level).unwrap_or(0));
        let end_margin = (end * (1.0 - CROSSFADE))..(end * (1.0 + CROSSFADE));
        levels.push(StrandLevel {
            level,
            keep: density * fraction,
            // One segment fewer per level, never below one: a strand is a curve
            // and a curve with no segments is nothing at all.
            segments: shape
                .segments()
                .saturating_sub(u32::try_from(level).unwrap_or(0))
                .max(1),
            // A quarter as many blades at twice the width cover the same
            // ground, which is what keeps a lawn the same colour across a
            // switch rather than thinning as the camera pulls back.
            width_scale: 1.0 / fraction.max(f32::MIN_POSITIVE).sqrt(),
            length_scale: if level == last && last > 0 {
                LAST_BAND_LENGTH
            } else {
                1.0
            },
            range: Some(VisibilityRange {
                start_margin: start.clone(),
                end_margin: end_margin.clone(),
                // The chunk's own origin, which `StrandChunk::origin` put in
                // the transform: a bound is a fact about the level and two
                // levels crossfading have to be measured from the same point.
                use_aabb: false,
            }),
        });
        start = end_margin;
    }
    levels
}

/// The scattered set of one layer, from the cache or freshly scattered into it.
///
/// Synchronous: the scatter runs on the calling thread, across
/// [`ScatterContext::threads`] of them. A repeat of sixty-five thousand strands
/// is a few milliseconds of field evaluation, and every surface wearing the
/// material after the first pays none of it.
#[cfg(feature = "strand-scatter")]
pub fn strand_set(
    graph: &str,
    layer: &str,
    params: &BTreeMap<String, ParamValue>,
    cx: &mut ScatterContext<'_>,
) -> Result<std::sync::Arc<StrandSet>> {
    let key = StrandKey::new(graph, layer, cx.field_resolution, params);
    if let Some(set) = cx.cache.get_strands(&key) {
        return Ok(std::sync::Arc::clone(set));
    }
    let set = ashlar_material::strands::scatter(&ashlar_material::strands::StrandRequest {
        graph: graph_of(graph, cx.graphs)?,
        library: cx.graphs,
        params,
        layer,
        field_resolution: cx.field_resolution,
        threads: cx.threads,
    })
    .with_context(|| format!("scattering strand layer {layer:?} of graph {graph:?}"))?;
    Ok(cx.cache.insert_strands(key, set))
}

/// Every layer a definition grows, scattered out of the graph its surface
/// names.
///
/// The tool half of [`StrandSets::read`], and what the content step that writes
/// a baked set calls before it writes one. The sets come back in the order the
/// definition names its layers, which is the order the file stores them in, so
/// that what a preview scatters and what a game reads are the same list.
///
/// Answers nothing where the definition grows no strands, or where its surface
/// names no graph.
///
/// # Errors
///
/// If the surface names a graph the library does not hold, or a layer does not
/// scatter. Both are refused by [`preflight_strands`] before a window exists.
#[cfg(feature = "strand-scatter")]
pub fn scatter_sets(
    definition: &ashlar::MaterialDefinition,
    cx: &mut ScatterContext<'_>,
) -> Result<Option<StrandSets>> {
    let Some(settings) = &definition.strands else {
        return Ok(None);
    };
    let Some(graph) = definition.surface.graph() else {
        return Ok(None);
    };
    let params = strand_params(&definition.surface);
    let mut sets = Vec::with_capacity(settings.layers.len());
    let mut origin = String::new();
    for layer in &settings.layers {
        let key = StrandKey::new(graph, layer, cx.field_resolution, params);
        // Every layer of one definition shares a graph, a resolution and a
        // parameter binding, so they share an origin; the layer is what
        // `CardKey` carries beside it.
        origin = key.origin();
        sets.push((*strand_set(graph, layer, params, cx)?).clone());
    }
    Ok(Some(StrandSets::of(origin, sets)))
}

/// Grow every strand layer a definition asks for on one meshed surface.
///
/// Answers nothing where the definition carries no [`ashlar::StrandSettings`],
/// which is nearly every material: a caller may hand every element it spawns to
/// this and pay a field read for the ones that grow nothing.
///
/// `sets` is where the strands come from — a baked file through
/// [`StrandSets::read`], or a graph through
/// `scatter_sets` — and this cannot tell which.
///
/// # Errors
///
/// If `sets` does not hold a layer the definition names. That is the *stale*
/// case: a baked set written before a layer was added to the graph, or one
/// written for another material. [`read_library`](crate::read_library) refuses it before a
/// window exists in a build that can check it, so a failure here is a file that
/// changed under a running game.
pub fn create_strands(
    definition: &ashlar::MaterialDefinition,
    surface: &ashlar::TriangleMesh,
    sets: &StrandSets,
    cx: &mut StrandContext<'_>,
) -> Result<Vec<StrandLayerMesh>> {
    let Some(settings) = &definition.strands else {
        return Ok(Vec::new());
    };
    let triangles = StrandSurface::of(surface, definition);
    let mut grown = Vec::new();
    for layer in &settings.layers {
        let set = sets.get(layer).with_context(|| {
            format!(
                "the baked set from {:?} holds no strand layer {layer:?}; it holds {}",
                sets.origin(),
                sets.declared()
            )
        })?;
        // Placed once at full detail and cut per level afterwards, because a
        // level is a rank *prefix*: the strands of a thinner level are the
        // strands of a thicker one, so planting them again would be the same
        // barycentric walk over the same triangles for a subset of the same
        // answer.
        let placed = place(set, &triangles.triangles(), settings.density);
        let cards = card_atlas(definition, sets.origin(), layer, set, settings, cx);
        grown.push(chunked(layer, set, &placed, settings, cards));
    }
    Ok(grown)
}

/// What building the card level of one layer takes.
struct CardBuild {
    /// Where the cards are drawn.
    range: VisibilityRange,
    /// What they are drawn and built at.
    settings: crate::cards::CardSettings,
    /// The picture they wear.
    atlas: crate::cards::CardImages,
    /// How much world one gather cell covers, which is the definition's own
    /// repeat divided by that lattice. The larger axis of a repeat that is not
    /// square, because this is a "no further apart than" and a tuft on the long
    /// axis is the widest one there is.
    cell_metres: f32,
}

/// The atlas one layer's cards wear, where the definition asks for cards.
///
/// Drawn once per distinct scatter and kept in the same cache the scatter is
/// in: the picture is the *set* seen from the side, so every surface wearing
/// the material binds the same two images however many walls it is.
fn card_atlas(
    definition: &ashlar::MaterialDefinition,
    origin: &str,
    layer: &str,
    set: &StrandSet,
    settings: &ashlar::StrandSettings,
    cx: &mut StrandContext<'_>,
) -> Option<CardBuild> {
    let range = crate::cards::band(settings)?;
    let cards = crate::cards::CardSettings::of(set);
    let repeat = definition.tile_metres[0]
        .max(definition.tile_metres[1])
        .abs();
    let build = |atlas| CardBuild {
        range,
        settings: cards,
        atlas,
        cell_metres: repeat / f32::from(u16::try_from(cards.cells.max(1)).unwrap_or(1)),
    };
    let key = crate::cards::CardKey::new(origin, layer, &cards, crate::cards::CARD_CUTOFF);
    if let Some(atlas) = cx.cards.get(&key) {
        return Some(build(atlas.clone()));
    }
    let levels = crate::cards::mips(&crate::cards::atlas(set, &cards), crate::cards::CARD_CUTOFF);
    let textures = crate::cards::textures(&levels);
    let atlas = cx.cards.insert(key, textures, cx.images);
    Some(build(atlas))
}

/// The parameter values a surface binds, or an empty map where it binds none.
#[cfg(feature = "strand-scatter")]
fn strand_params(surface: &ashlar::Surface) -> &BTreeMap<String, ashlar::ParamValue> {
    const NONE: &BTreeMap<String, ashlar::ParamValue> = &BTreeMap::new();
    match surface {
        ashlar::Surface::Graph(bake)
        | ashlar::Surface::Files {
            baked_from: Some(bake),
            ..
        } => &bake.params,
        ashlar::Surface::Shader { params, .. } => params,
        ashlar::Surface::Plain | ashlar::Surface::Files { .. } => NONE,
    }
}

/// The planted strands of one layer, by level and then by repeat.
fn chunked(
    layer: &str,
    set: &StrandSet,
    placed: &[PlacedStrand],
    settings: &ashlar::StrandSettings,
    cards: Option<CardBuild>,
) -> StrandLayerMesh {
    // A `BTreeMap` rather than a hash map: the chunks come back in repeat order
    // whatever order placement walked the triangles in, so the entities a
    // caller spawns from them are the same entities every run.
    let mut repeats: BTreeMap<[i32; 2], Vec<PlacedStrand>> = BTreeMap::new();
    let mut roughness = 0.0_f64;
    for strand in placed {
        repeats.entry(strand.repeat).or_default().push(*strand);
        roughness += f64::from(strand.strand.roughness);
    }
    // One origin per chunk, from the strands at *full* detail, so every level
    // of a chunk is drawn about the same point however many strands it kept.
    let origins: BTreeMap<[i32; 2], [f32; 3]> = repeats
        .iter()
        .map(|(repeat, strands)| (*repeat, centroid(strands)))
        .collect();
    let mut triangles = 0;
    let mut levels = Vec::new();
    for level in strand_levels(settings, set.shape()) {
        let shape = level.shape(set.shape());
        let mut chunks = Vec::with_capacity(repeats.len());
        for (repeat, strands) in &repeats {
            let origin = origins.get(repeat).copied().unwrap_or_default();
            let kept: Vec<PlacedStrand> = strands
                .iter()
                .filter(|placed| placed.strand.rank < level.keep)
                .map(|placed| level.scaled(placed, origin))
                .collect();
            let built = mesh(&kept, shape);
            triangles += built.triangles();
            chunks.push(StrandChunk {
                repeat: *repeat,
                origin,
                strands: kept.len(),
                mesh: strand_mesh(&built),
            });
        }
        levels.push(StrandLevelMesh { level, chunks });
    }
    // The cards are built from every planted strand rather than from a level's
    // survivors: a card is a picture of the whole tuft, and the thinning above
    // is what a card replaces rather than what it is drawn from.
    let cards = cards.map(|build| {
        let mut chunks = Vec::with_capacity(repeats.len());
        for (repeat, strands) in &repeats {
            let origin = origins.get(repeat).copied().unwrap_or_default();
            let built = crate::cards::mesh(strands, &build.settings, build.cell_metres, origin);
            triangles += built.triangles;
            chunks.push(StrandChunk {
                repeat: *repeat,
                origin,
                strands: built.cards,
                mesh: built.mesh,
            });
        }
        StrandCardMesh {
            range: build.range,
            chunks,
            atlas: build.atlas,
            cutoff: crate::cards::CARD_CUTOFF,
            settings: build.settings,
        }
    });
    StrandLayerMesh {
        layer: layer.to_owned(),
        levels,
        cards,
        strands: placed.len(),
        triangles,
        roughness: mean(roughness, placed.len()),
    }
}

/// The mean root of a chunk's strands, which is the point its levels are drawn
/// about.
///
/// The mean rather than the centre of the bounds, because a bound is a fact
/// about the *level* — a thinner level has a different one — and the whole
/// point of this origin is that every level of a chunk shares it.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "a strand count is bounded by the lattice, and a mean of positions \
              is a renderer coordinate, which is where f64 becomes f32"
)]
fn centroid(strands: &[PlacedStrand]) -> [f32; 3] {
    if strands.is_empty() {
        return [0.0; 3];
    }
    let mut total = [0.0_f64; 3];
    for strand in strands {
        for (sum, axis) in total.iter_mut().zip(strand.position) {
            *sum += f64::from(axis);
        }
    }
    let count = strands.len() as f64;
    [
        (total[0] / count) as f32,
        (total[1] / count) as f32,
        (total[2] / count) as f32,
    ]
}

/// The mean of a sum, or the roughness a strand layer defaults to where there
/// were none to average.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "a strand count is bounded by the lattice, and a mean of values in 0..=1 is one"
)]
fn mean(total: f64, count: usize) -> f32 {
    if count == 0 {
        return 0.8;
    }
    (total / count as f64) as f32
}

/// One built layer as a Bevy mesh.
///
/// [`RenderAssetUsages::default`] rather than the render world alone, which is
/// what every other mesh in this crate uses and what Bevy's own bounds
/// calculation reads: a chunk with no main-world copy has no `Aabb`, and an
/// entity with no `Aabb` is never culled. The copy is the price of the culling
/// the chunking exists for.
fn strand_mesh(built: &ashlar_strands::StrandMesh) -> Mesh {
    Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, built.positions.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, built.normals.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, built.uvs.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, built.colors.clone())
    .with_inserted_attribute(ATTRIBUTE_STRAND_WIND, built.wind.clone())
    .with_inserted_indices(bevy::mesh::Indices::U32(built.indices.clone()))
}

/// The material a strand layer draws through.
///
/// A `StandardMaterial` and nothing more, which is the whole of decision 9's
/// bargain: strands cost draw calls and cost no render-graph code.
///
/// Four things on it are not the default and each is a fact about a blade.
/// Backface culling is **off** and the material is double sided, because a
/// ribbon has no inside and a blade seen from behind is a blade. The base
/// colour is white, because the colour is the mesh's — a strand carries its own
/// root-to-tip gradient as a vertex colour, and a constant over it would make
/// every layer of every material one colour. The roughness is the mean of the
/// strands actually grown; [`StrandLayerMesh::roughness`] says why.
pub fn strand_material(
    definition: &ashlar::MaterialDefinition,
    roughness: f32,
) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: roughness.clamp(0.0, 1.0),
        metallic: 0.0,
        // A blade is thinner than a shadow texel from any distance a shadow map
        // covers, so a layer drawn with culling on flickers as the camera moves
        // past the plane of it. Off is not an optimisation being skipped; it is
        // the only setting that draws grass.
        cull_mode: None,
        double_sided: true,
        // A blade is a leaf, and a leaf lit from behind glows rather than going
        // black. Without this the face turned away from the key light gets
        // ambient alone, which at a grazing angle is most of the blades in
        // frame and reads as soot over the pile. A quarter is enough to lift
        // the shaded side to something a lawn plausibly does and little enough
        // that a blade still has a lit side and a dark one.
        diffuse_transmission: BLADE_TRANSMISSION,
        emissive: LinearRgba::rgb(
            definition.emissive[0],
            definition.emissive[1],
            definition.emissive[2],
        ),
        ..default()
    }
}

/// The same material with the wind vertex stage over it.
///
/// What a caller that has added [`StrandPlugin`](crate::wind::StrandPlugin)
/// spawns: an [`ExtendedMaterial`](bevy::pbr::ExtendedMaterial) is a different
/// asset type from a `StandardMaterial`, so which of the two a scene uses is a
/// decision it makes once and then holds to for every chunk.
///
/// The base half is [`strand_material`] exactly, and at
/// [`StrandWind::still`](crate::wind::StrandWind::still) the vertex stage
/// displaces by zero before it computes anything, so the two draw the same
/// picture.
pub fn strand_wind_material(
    definition: &ashlar::MaterialDefinition,
    roughness: f32,
) -> crate::wind::StrandWindMaterial {
    bevy::pbr::ExtendedMaterial {
        base: strand_material(definition, roughness),
        extension: crate::wind::StrandWindExtension::default(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "a level's cut and its scales are the constants above, written \
                  out; a margin would pass a level that had quietly stopped \
                  being either"
    )]
    use super::*;

    /// The tool half's key, which only a build that can scatter one has.
    #[cfg(feature = "strand-scatter")]
    fn values(seed: f32) -> BTreeMap<String, ParamValue> {
        let mut params = BTreeMap::new();
        params.insert("lushness".to_owned(), ParamValue::Float(seed));
        params
    }

    #[cfg(feature = "strand-scatter")]
    #[test]
    fn a_strand_key_is_the_graph_the_layer_the_resolution_and_every_value() {
        let key = |graph: &str, layer: &str, fields: u32, seed: f32| {
            StrandKey::new(graph, layer, fields, &values(seed))
        };
        let one = key("benchmark:grass", "blades", STRAND_FIELDS, 0.5);
        assert_eq!(one, key("benchmark:grass", "blades", STRAND_FIELDS, 0.5));
        // Each of the four is load-bearing, and a key that dropped any of them
        // would hand one lawn another's blades.
        for other in [
            key("benchmark:moss", "blades", STRAND_FIELDS, 0.5),
            key("benchmark:grass", "stems", STRAND_FIELDS, 0.5),
            key("benchmark:grass", "blades", 256, 0.5),
            key("benchmark:grass", "blades", STRAND_FIELDS, 0.25),
        ] {
            assert_ne!(one, other, "{other:?}");
        }
        // A parameter nobody bound is not the same as one bound at a value, and
        // an empty map is its own key rather than a wildcard.
        assert_ne!(
            one,
            StrandKey::new("benchmark:grass", "blades", STRAND_FIELDS, &BTreeMap::new())
        );
        assert_eq!(one.graph(), "benchmark:grass");
        assert_eq!(one.layer(), "blades");
        assert_eq!(one.field_resolution(), STRAND_FIELDS);
    }

    #[cfg(feature = "strand-scatter")]
    #[test]
    fn equal_keys_hash_equally_and_a_cache_keyed_by_one_finds_it_again() {
        use std::collections::HashMap;
        use std::hash::{BuildHasher, RandomState};

        let state = RandomState::new();
        let one = StrandKey::new("benchmark:grass", "blades", STRAND_FIELDS, &values(0.5));
        let same = StrandKey::new("benchmark:grass", "blades", STRAND_FIELDS, &values(0.5));
        assert_eq!(state.hash_one(&one), state.hash_one(&same));

        let mut cache: HashMap<StrandKey, &str> = HashMap::new();
        cache.insert(one, "the lawn");
        assert_eq!(cache.get(&same).copied(), Some("the lawn"));
        assert!(
            !cache.contains_key(&StrandKey::new(
                "benchmark:grass",
                "blades",
                STRAND_FIELDS,
                &values(0.25)
            )),
            "a different value is a different lawn"
        );
    }

    /// The settings a definition carries, with the distances named.
    fn settings(lod: &[f32], density: f32) -> ashlar::StrandSettings {
        ashlar::StrandSettings::new(["blades"])
            .lod_metres(lod.iter().copied())
            .density(density)
    }

    fn shape(segments: u32) -> ashlar_strands::StrandShape {
        ashlar_strands::StrandShape {
            segments,
            ..Default::default()
        }
    }

    #[test]
    fn a_layer_that_names_no_distance_is_one_level_that_is_always_drawn() {
        let levels = strand_levels(&settings(&[], 0.5), shape(3));
        assert_eq!(levels.len(), 1);
        assert!(levels[0].range.is_none(), "nothing to fade between");
        assert_eq!(levels[0].keep, 0.5, "the definition's own density");
        assert_eq!(levels[0].width_scale, 1.0);
        assert_eq!(levels[0].length_scale, 1.0);
    }

    #[test]
    fn each_level_keeps_a_prefix_of_the_one_before_it_and_widens_by_one_over_its_root() {
        // The two halves of the bargain, and the reason the factor is not a
        // tuned number: a quarter as many blades at twice the width cover the
        // same ground, so a lawn does not thin as the camera pulls back.
        let levels = strand_levels(&settings(&[2.5, 7.0], 1.0), shape(3));
        assert_eq!(levels.len(), 2, "one level per distance");
        assert_eq!(levels[0].keep, 1.0);
        assert_eq!(levels[1].keep, LEVEL_KEEP);
        assert!(
            levels[1].keep < levels[0].keep,
            "a level is a prefix of the one before it"
        );
        let widened = 1.0 / LEVEL_KEEP.sqrt();
        assert!(
            (levels[1].width_scale - widened).abs() < 1e-6,
            "{} is not 1/sqrt({LEVEL_KEEP})",
            levels[1].width_scale
        );
        // Fewer segments, never below one.
        assert_eq!(levels[0].segments, 3);
        assert_eq!(levels[1].segments, 2);
        assert_eq!(
            strand_levels(&settings(&[1.0], 1.0), shape(1))[0].segments,
            1
        );
        // And the last band is where the blades sink into the relief rather
        // than popping out of it.
        assert_eq!(levels[0].length_scale, 1.0);
        assert_eq!(levels[1].length_scale, LAST_BAND_LENGTH);
    }

    #[test]
    fn the_bands_meet_so_that_one_levels_fade_out_is_the_nexts_fade_in() {
        // Bevy's crossfade only works when the two margins are the same range,
        // which is what makes a switch a dither rather than a pop.
        let levels = strand_levels(&settings(&[2.5, 7.0], 1.0), shape(3));
        let ranges: Vec<&VisibilityRange> = levels
            .iter()
            .filter_map(|level| level.range.as_ref())
            .collect();
        assert_eq!(ranges.len(), 2);
        assert_eq!(
            ranges[0].start_margin,
            0.0..0.0,
            "level zero starts at the eye"
        );
        assert_eq!(ranges[0].end_margin, ranges[1].start_margin);
        assert!(ranges[1].end_margin.start > ranges[1].start_margin.end);
        // Past the last band there is no level at all, which is the relief
        // taking over.
        assert!(!ranges[1].is_visible_at_all(9.0));
        assert!(
            !ranges[0].use_aabb,
            "every level is measured from one point"
        );
    }

    #[test]
    fn a_levels_density_is_the_definitions_own_cut_and_not_a_second_one() {
        // A scene that already thinned its lawn keeps that cut at every level:
        // the level factors multiply the definition's density rather than
        // replacing it, or asking for a third of a lawn would quietly grow all
        // of it at the far level.
        let levels = strand_levels(&settings(&[2.0, 6.0], 0.36), shape(2));
        assert!((levels[0].keep - 0.36).abs() < 1e-6);
        assert!((levels[1].keep - 0.36 * LEVEL_KEEP).abs() < 1e-6);
    }

    #[test]
    fn a_surface_is_measured_in_repeats_of_the_material_rather_than_in_metres() {
        let mut surface = ashlar::TriangleMesh::default();
        surface.push_triangle(
            [
                ashlar::glam::DVec3::ZERO,
                ashlar::glam::DVec3::new(4.0, 0.0, 0.0),
                ashlar::glam::DVec3::new(4.0, 0.0, 4.0),
            ],
            [ashlar::glam::DVec3::Y; 3],
            [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]],
            ashlar::FaceSource::BODY,
        );
        let definition = ashlar::MaterialDefinition {
            tile_metres: [2.0, 2.0],
            uv_offset: [0.25, 0.0],
            ..Default::default()
        };
        let converted = StrandSurface::of(&surface, &definition);
        // Four metres of UV over a two-metre repeat is two repeats, and the
        // offset is already in repeats, so it is added rather than divided.
        assert_eq!(
            converted.triangles().uvs,
            [[0.25, 0.0], [2.25, 0.0], [2.25, 2.0]]
        );
    }
}
