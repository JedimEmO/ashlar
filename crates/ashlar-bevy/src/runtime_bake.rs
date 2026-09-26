//! Baking a material graph into `Image` assets, and the cache that keeps them.
//!
//! A [`ashlar::Surface::Graph`] definition names a graph, parameter values and
//! a resolution instead of files. This module runs that bake when the material
//! is created and hands the encoded maps to `Assets<Image>` with the formats,
//! the mip chain and the repeat sampling a wall needs, so a runtime-baked
//! surface is the same picture as the KTX2 files a content step would have
//! written for the same [`ashlar::Bake`] — the same bytes, level for level.
//!
//! Three things are worth knowing before leaning on it.
//!
//! **It is not free.** A bake is CPU work measured in hundreds of milliseconds
//! at 1024 squared, and the mip chain is a quarter again on top. Called from
//! [`create_graph_material`] it is synchronous and blocks whatever thread it is on,
//! which for a preview or a test is exactly right and for a game at play is
//! not: [`bake_images`] is the same work as a pure function that touches no
//! `World`, so it can run in an [`AsyncComputeTaskPool`](bevy::tasks::AsyncComputeTaskPool)
//! task and only the cheap half — [`BakeCache::insert`], which moves the
//! finished images into `Assets<Image>` — has to happen on the main thread.
//!
//! **Every distinct bake is its own texture set in memory.** That is the point
//! of the path: a per-building seed costs no files. It also means a hundred
//! seeds are a hundred sets, so [`BakeCache`] keys by the bake itself and two
//! definitions asking for the same one share a set.
//!
//! **A broken graph is a startup error, not a bake failure.**
//! [`read_library`](crate::read_library) preflights every `Graph` surface by validating and
//! lowering it against the same graph library, so by the time a bake runs the
//! only thing left that can go wrong is an expression the interpreter cannot
//! evaluate.
use std::{collections::HashMap, num::NonZeroUsize};
// The strand cache hands one scattered set to every chunk that reads it.
#[cfg(feature = "strand-scatter")]
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use ashlar_material::{
    MaterialGraph, MaterialGraphLibrary,
    bake::{BakeRequest, Encoded, PlaneFormat, TextureSet},
};
// The identity of one plane, which only a compiled graph's bound images are
// keyed by.
#[cfg(feature = "shader")]
use ashlar_material::planes::PlaneKey;
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDataOrder, TextureDimension, TextureFormat},
};

use crate::{Maps, standard};

/// The graph a surface names, or an error listing the keys the library does
/// hold.
pub(crate) fn graph_of<'a>(
    key: &str,
    graphs: &'a MaterialGraphLibrary,
) -> Result<&'a MaterialGraph> {
    graphs.get(key).with_context(|| {
        format!(
            "unknown material graph {key:?}; the library holds {}",
            graphs
                .graphs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// How a baked plane's format reads on the GPU.
///
/// Four formats in and four out, with no transcoding in between: these are the
/// same four [`ktx2::write`](ashlar_material::ktx2::write) puts in a file and
/// Bevy's own loader reads back out of one, which is what makes a runtime bake
/// and a shipped file the same texture.
pub const fn texture_format(format: PlaneFormat) -> TextureFormat {
    match format {
        PlaneFormat::Rgba8Srgb => TextureFormat::Rgba8UnormSrgb,
        PlaneFormat::Rgba8Unorm => TextureFormat::Rgba8Unorm,
        PlaneFormat::R16Unorm => TextureFormat::R16Unorm,
        PlaneFormat::Rgba16Float => TextureFormat::Rgba16Float,
    }
}

/// One encoded map as an image, with its whole mip chain and a repeating
/// trilinear sampler.
///
/// `resolution` is level 0's edge, as [`TextureSet::resolution`] carries it;
/// level `n` of `encoded` must be `(resolution >> n).max(1)` squared, which is
/// what a bake writes and what [`ktx2::write`](ashlar_material::ktx2::write)
/// checks before it writes a file. Bevy takes the levels concatenated
/// largest-first — [`TextureDataOrder::MipMajor`] — which is the order
/// [`Encoded::mips`] is already in.
///
/// The sampler is the one [`create_material`](crate::create_material) asks Bevy's loader for on a
/// file map, so a wall does not change how it filters when its surface moves
/// between files and a runtime bake: repeat in both axes, linear
/// minification, magnification and mip interpolation. Trilinear rather than
/// nearest-mip is what stops the chain this bake just built from showing as a
/// visible ring on the ground.
///
/// [`RenderAssetUsages::RENDER_WORLD`] alone: these texels exist to be
/// uploaded, and keeping the main-world copy would double a texture set that
/// is already several megabytes per material. A caller who wants the numbers
/// back has them in the [`TextureSet`] this was made from.
pub fn image(encoded: &Encoded, resolution: u32) -> Image {
    for (level, bytes) in encoded.mips.iter().enumerate() {
        // The caller's contract, and a bake keeps it. Checked rather than
        // trusted because a chain whose lengths do not add up is a texture the
        // GPU reads as garbage rather than a texture wgpu rejects.
        let edge = usize::try_from((resolution >> level).max(1)).unwrap_or(usize::MAX);
        debug_assert_eq!(
            bytes.len(),
            edge * edge * encoded.format.bytes_per_texel(),
            "level {level} is not {edge} texels squared, which is what this map's \
             own resolution says it is"
        );
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        texture_format(encoded.format),
        RenderAssetUsages::RENDER_WORLD,
    );
    // A chain over the largest bake is thirteen levels, so the fallback is
    // unreachable; it is here rather than an unwrap because a cast that can
    // lie is worse than a saturation nobody reaches.
    image.texture_descriptor.mip_level_count =
        u32::try_from(encoded.mips.len()).unwrap_or(u32::MAX);
    image.data_order = TextureDataOrder::MipMajor;
    image.data = Some(encoded.mips.concat());
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        min_filter: ImageFilterMode::Linear,
        mag_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        // The same filtering a `Files` surface gets, and for the reason
        // [`crate::ANISOTROPY`] gives: a runtime bake and a loaded file are the
        // same maps and must not be sampled two different ways.
        anisotropy_clamp: crate::ANISOTROPY,
        ..default()
    });
    image
}

/// A baked texture set as images, before they are assets.
///
/// What [`bake_images`] answers and [`BakeCache::insert`] consumes: the split
/// exists so that the expensive half can run anywhere and only the handing-over
/// needs `Assets<Image>` and the main thread.
#[derive(Debug)]
pub struct GraphTextures {
    /// sRGB base colour.
    pub base_color: Image,
    /// Linear tangent-space normal, derived from the height plane.
    pub normal: Image,
    /// Linear occlusion, roughness and metallic packed into `R`, `G`, `B`.
    pub orm: Image,
    /// The sixteen-bit height plane, where the graph binds one.
    pub height: Option<Image>,
    /// Linear HDR emissive, where the graph binds one.
    pub emissive: Option<Image>,
}

impl From<&TextureSet> for GraphTextures {
    fn from(set: &TextureSet) -> Self {
        let map = |encoded| image(encoded, set.resolution);
        Self {
            base_color: map(&set.base_color),
            normal: map(&set.normal),
            orm: map(&set.orm),
            height: set.height.as_ref().map(map),
            emissive: set.emissive.as_ref().map(map),
        }
    }
}

/// The handles one baked texture set contributes, once its images are assets.
///
/// Cheap to clone, which is what sharing a set between definitions comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphImages {
    /// sRGB base colour.
    pub base_color: Handle<Image>,
    /// Linear tangent-space normal.
    pub normal: Handle<Image>,
    /// Linear occlusion, roughness and metallic. One image fills both the
    /// metallic-roughness and the occlusion slot, as a file ORM map does.
    pub orm: Handle<Image>,
    /// The height plane, where the graph binds one. Kept because it is baked
    /// and addressable, and deliberately bound to no
    /// [`StandardMaterial`] slot; see [`create_graph_material`].
    pub height: Option<Handle<Image>>,
    /// Linear HDR emissive, where the graph binds one.
    pub emissive: Option<Handle<Image>>,
}

/// One parameter value as the bits that decide whether two bakes are the same
/// bake.
///
/// `f32` is not [`Eq`], and a key has to be, so the floats are compared by
/// their bit patterns. That is conservative in exactly one direction: `0.0` and
/// `-0.0` are one value and two keys, which costs a duplicate texture set and
/// never hands a material somebody else's texels. `NaN` never arrives, because
/// [`ashlar::MaterialDefinition::check`] refuses a non-finite parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ParamBits {
    /// One channel.
    Float(u32),
    /// Linear RGB.
    Color([u32; 3]),
    /// A count, a seed or an integer period.
    Int(i32),
    /// A switch.
    Bool(bool),
}

impl ParamBits {
    /// Feed this value to a digest, tagged so that two variants holding the
    /// same bits are two values.
    // Only a [`ShaderKey`] is ever digested; a bake key is compared whole.
    #[cfg(feature = "shader")]
    fn hash_into(self, hash: &mut Fnv) {
        match self {
            Self::Float(bits) => {
                hash.number(0);
                hash.number(u64::from(bits));
            }
            Self::Color(bits) => {
                hash.number(1);
                for lane in bits {
                    hash.number(u64::from(lane));
                }
            }
            Self::Int(value) => {
                hash.number(2);
                hash.number(u64::from(value.cast_unsigned()));
            }
            Self::Bool(value) => {
                hash.number(3);
                hash.number(u64::from(value));
            }
        }
    }
}

impl From<&ashlar::ParamValue> for ParamBits {
    fn from(value: &ashlar::ParamValue) -> Self {
        match *value {
            ashlar::ParamValue::Float(value) => Self::Float(value.to_bits()),
            ashlar::ParamValue::Color(value) => Self::Color(value.map(f32::to_bits)),
            ashlar::ParamValue::Int(value) => Self::Int(value),
            ashlar::ParamValue::Bool(value) => Self::Bool(value),
        }
    }
}

/// What makes two [`ashlar::Bake`]s the same bake: the graph key, every
/// parameter value, and the resolution.
///
/// The whole bake rather than a hash of it. A 64-bit digest would key the cache
/// just as well until the day two bakes collided, and the failure then is not a
/// slow frame but one building silently wearing another's textures, for one
/// pair of parameter sets somewhere in a content library. An owned key is a
/// string and a handful of words per material, which is nothing beside the
/// megabytes it is protecting.
///
/// Every key in this cache follows that policy, and each one says which form of
/// it: [`ShaderKey`] is the graph and its values, and [`PlaneImageKey`] is an
/// exact structural key — a plane's whole canonical encoding, compared byte for
/// byte — rather than a digest with an equality check behind it. That last one
/// is the only key here that is not obviously cheap: a plane key is a few
/// kilobytes over a shallow graph but a hundred over a deep one, and
/// [`PlaneKey`] gives the measured sizes. It is still well under a per cent of
/// the texels it protects. What is not a matter of taste is a digest that is
/// *believed*, and none of the three is one.
///
/// It names a graph rather than describing one, so a library reloaded in place
/// with the same keys and different graphs must
/// [`clear`](BakeCache::clear) the cache: the key would not have changed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BakeKey {
    /// Content key of the graph.
    graph: String,
    /// Texels per repeat.
    resolution: u32,
    /// Every bound parameter, in the [`std::collections::BTreeMap`] order the
    /// bake carries them in, so the same map is always the same key.
    params: Vec<(String, ParamBits)>,
}

impl BakeKey {
    /// The key of one bake.
    pub fn new(bake: &ashlar::Bake) -> Self {
        Self {
            graph: bake.graph.clone(),
            resolution: bake.resolution,
            params: bake
                .params
                .iter()
                .map(|(name, value)| (name.clone(), ParamBits::from(value)))
                .collect(),
        }
    }

    /// The graph this key is for.
    ///
    /// What tells the three texture sets of one wall key from the trim beside
    /// them: an instance override changes the values and never the graph, so a
    /// block of buildings over one material is several keys naming one graph.
    pub fn graph(&self) -> &str {
        &self.graph
    }

    /// Texels per repeat of the maps it holds.
    pub fn resolution(&self) -> u32 {
        self.resolution
    }
}

impl From<&ashlar::Bake> for BakeKey {
    fn from(bake: &ashlar::Bake) -> Self {
        Self::new(bake)
    }
}

/// What makes two compiled graphs the same compiled graph: the graph key, the
/// texel count, which parameters stayed live, and the value of every parameter
/// that did not.
///
/// [`BakeKey`]'s policy, applied to a [`ashlar::Surface::Shader`]. The exact
/// values rather than a digest of them, for the same reason: a collision here
/// would not be a slow frame but one wall wearing another's textures and
/// another's shader.
///
/// The two halves of the key are two because they do two different things. The
/// *live* names decide what the generated WGSL reads out of the uniform block,
/// and their values do not — that is the whole point of a live parameter. The
/// *folded* values decide what is baked into the bound textures and what is
/// written into the shader as a literal, so two definitions that fold a
/// parameter differently are two different compiled graphs even though they
/// name one graph.
///
/// Like [`BakeKey`], it names a graph rather than describing one, so a library
/// reloaded in place with the same keys and different graphs must
/// [`clear`](BakeCache::clear) the cache and
/// [`GraphShaders::clear`](crate::shader::GraphShaders::clear) the registry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg(feature = "shader")]
pub struct ShaderKey {
    /// Content key of the graph.
    graph: String,
    /// Texels per repeat of the bound textures.
    resolution: u32,
    /// Parameters the shader reads from its uniform block, in the order the
    /// block declares them.
    live: Vec<String>,
    /// Every other parameter and the value it was folded at, in name order.
    folded: Vec<(String, ParamBits)>,
}

#[cfg(feature = "shader")]
impl ShaderKey {
    /// The key of one partitioned graph.
    ///
    /// `material` is the graph with the definition's parameter values already
    /// bound, which is what makes the folded half exact: a parameter the
    /// definition did not mention is keyed at the default it will actually be
    /// folded at rather than at nothing.
    pub fn new(
        graph: &str,
        resolution: u32,
        material: &ashlar_material::Material,
        split: &ashlar_material::partition::Partition,
    ) -> Self {
        let live: Vec<String> = split.live().to_vec();
        let folded = material
            .graph()
            .params
            .iter()
            .filter(|param| !live.contains(&param.name))
            .map(|param| (param.name.clone(), ParamBits::from(&param.value)))
            .collect();
        Self {
            graph: graph.to_owned(),
            resolution,
            live,
            folded,
        }
    }

    /// The graph this key is for.
    pub fn graph(&self) -> &str {
        &self.graph
    }

    /// Texels per repeat of its bound textures.
    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// The parameters that stayed live, in uniform-block order.
    pub fn live(&self) -> &[String] {
        &self.live
    }

    /// Sixty-four bits of this key, for the places a key has to be [`Copy`].
    ///
    /// FNV-1a over every field, written out rather than taken from a `Hasher`
    /// so that the same key gives the same number in every process — a shader
    /// asset id is derived from it, and an id that moved between runs would be
    /// impossible to recognise in a log.
    pub fn digest(&self) -> u64 {
        let mut hash = Fnv::new();
        hash.text(&self.graph);
        hash.number(u64::from(self.resolution));
        hash.number(self.live.len() as u64);
        for name in &self.live {
            hash.text(name);
        }
        hash.number(self.folded.len() as u64);
        for (name, bits) in &self.folded {
            hash.text(name);
            bits.hash_into(&mut hash);
        }
        hash.finish()
    }

    /// The key as one line, for an error that has to name two of them.
    pub fn describe(&self) -> String {
        format!(
            "{} at {} texels, live [{}]",
            self.graph,
            self.resolution,
            self.live.join(", ")
        )
    }
}

/// What makes two bound images of two compiled graphs the same image: the
/// format it is uploaded in, and the identity of the plane in every lane of it.
///
/// A [`ShaderKey`] says whether two *materials* are the same material; this
/// says whether two *textures* are the same texture, which is a finer question
/// and the one a bake should be charged for. `showcase:concrete-wet` and
/// `showcase:concrete-cut-aware` are two shader keys — different live sets,
/// different fragment work — over one wall: both instance the same concrete
/// through `Subgraph` and both bind the same eight planes of it. Keyed by the
/// shader, that wall is rasterised twice; keyed by the plane behind each image,
/// once.
///
/// **The exact key, and not a digest of it**, which is
/// [`BakeKey`]'s policy applied one level down and argued for there at length:
/// a collision would not be a slow frame but one building wearing another's
/// texels. [`PlaneKey`] is the canonical encoding of the plane's own
/// sub-expression after folding — its filter chain, its resolution and its
/// value type with it — so two keys are equal exactly when the planes are, and
/// the comparison is a byte comparison rather than a digest that has to be
/// believed. What that costs is the size of the graph above the filter rather
/// than a fixed few kilobytes — about five kilobytes for a plane of a plain
/// concrete graph and a hundred for one of a painted-metal graph, against four
/// megabytes of `f32` a scalar plane is at 1024. [`PlaneKey`] has the table.
///
/// What it deliberately does *not* name is the graph. Two planes with the same
/// expression in two libraries are the same picture, and a key that named the
/// graph would say they were not — which is the whole of the sharing this
/// exists for. A library reloaded in place is still safe, because an edit
/// changes the expression and so changes the key, and
/// [`BakeCache::clear`] is what a reload calls anyway.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg(feature = "shader")]
pub struct PlaneImageKey {
    /// The format the image is uploaded in. A property of the image rather
    /// than of any one plane in it, so it is keyed here.
    format: PlaneFormat,
    /// The first channel each packed plane occupies, and which plane it is, in
    /// the order the binding lists them.
    lanes: Vec<(u8, PlaneKey)>,
}

#[cfg(feature = "shader")]
impl PlaneImageKey {
    /// The key of one bound image: its format, and the lane each plane in it
    /// was written into.
    pub fn new(format: PlaneFormat, lanes: impl IntoIterator<Item = (u8, PlaneKey)>) -> Self {
        Self {
            format,
            lanes: lanes.into_iter().collect(),
        }
    }

    /// How many planes are packed into it. One for the ordinary case, up to
    /// four where scalars were packed.
    pub fn planes(&self) -> usize {
        self.lanes.len()
    }
}

/// FNV-1a, as `ashlar-material`'s plane keys use it.
///
/// Sixty-four bits and no dependency. It is a digest of a key that is checked
/// against the full key wherever it is used, not a key.
#[cfg(feature = "shader")]
struct Fnv(u64);

#[cfg(feature = "shader")]
impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn byte(&mut self, byte: u8) {
        self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }

    fn number(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.byte(byte);
        }
    }

    fn text(&mut self, text: &str) {
        self.number(text.len() as u64);
        for byte in text.as_bytes() {
            self.byte(*byte);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

/// The texture sets runtime bakes have produced, keyed by the bake that
/// produced them.
///
/// This is the resource the design's "two buildings asking for the same thing
/// share one set" comes to. Insert it with `App::init_resource`, and take it as
/// `ResMut<BakeCache>` wherever materials are created.
///
/// Nothing evicts. A cache holds whole texture sets, so a game that bakes many
/// distinct parameter sets over a long session should drop the ones it is done
/// with — [`clear`](Self::clear) is the blunt instrument, and dropping the
/// handles is what releases the images.
#[derive(Resource, Debug, Default)]
pub struct BakeCache {
    /// One entry per distinct bake.
    sets: HashMap<BakeKey, GraphImages>,
    /// One entry per distinct compiled graph: the bound textures its shader
    /// samples, in binding order.
    ///
    /// The same cache rather than a second resource, because it is the same
    /// thing being cached — texels a graph produced, kept so two definitions
    /// that asked for the same ones share them — and because a caller that has
    /// to clear one has to clear the other.
    ///
    /// A memo over [`Self::images`] rather than a store: the handles here are
    /// the handles there, looked up once per compiled graph so that a material
    /// created again pays neither the rasterisation nor the lookup.
    #[cfg(feature = "shader")]
    statics: HashMap<ShaderKey, Vec<Handle<Image>>>,
    /// One entry per distinct bound image, keyed by the plane behind it rather
    /// than by the graph that asked for it.
    ///
    /// This is what makes two compiled graphs over one wall bake that wall
    /// once. [`PlaneImageKey`] says why it is safe to share across graphs.
    #[cfg(feature = "shader")]
    images: HashMap<PlaneImageKey, Handle<Image>>,
    /// One entry per distinct strand scatter.
    ///
    /// Beside the texels rather than in a resource of its own, for the reason
    /// [`Self::statics`] is here: it is the same thing being cached — an answer
    /// a graph produced, kept so two surfaces asking for the same one share it
    /// — and a caller that has to clear one has to clear the other.
    ///
    /// Behind an [`Arc`] because a set is a list of sixty-five thousand
    /// strands and every level of detail of every chunk of every wall wearing
    /// the material reads the same one. Placement borrows it; nothing mutates
    /// one after it is here.
    #[cfg(feature = "strand-scatter")]
    strands: HashMap<crate::strands::StrandKey, Arc<ashlar_material::strands::StrandSet>>,
    /// How many planes have been rasterised for compiled graphs, cumulative.
    ///
    /// The number this cache exists to keep down, and the only way to see it
    /// from outside: a bound image that was already here costs nothing and
    /// prints nothing, so a count of cache entries cannot tell a cheap
    /// material from an expensive one.
    #[cfg(feature = "shader")]
    rasterised: usize,
}

impl BakeCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The images of a bake this cache has already run, if it has.
    pub fn get(&self, key: &BakeKey) -> Option<&GraphImages> {
        self.sets.get(key)
    }

    /// Hand a finished bake to `images` and remember its handles.
    ///
    /// The main-thread half of an off-thread bake, and the only half that
    /// touches the ECS. Re-inserting a key replaces the entry, which is what a
    /// re-bake of an edited graph wants; the previous images go when the last
    /// handle to them does.
    pub fn insert(
        &mut self,
        key: BakeKey,
        textures: GraphTextures,
        images: &mut Assets<Image>,
    ) -> GraphImages {
        let baked = GraphImages {
            base_color: images.add(textures.base_color),
            normal: images.add(textures.normal),
            orm: images.add(textures.orm),
            height: textures.height.map(|image| images.add(image)),
            emissive: textures.emissive.map(|image| images.add(image)),
        };
        self.sets.insert(key, baked.clone());
        baked
    }

    /// The bound textures of a compiled graph this cache has already baked, if
    /// it has.
    #[cfg(feature = "shader")]
    pub fn get_static(&self, key: &ShaderKey) -> Option<&[Handle<Image>]> {
        self.statics.get(key).map(Vec::as_slice)
    }

    /// Remember which images a compiled graph binds, in binding order.
    ///
    /// The handles come from [`insert_image`](Self::insert_image) or from
    /// [`get_image`](Self::get_image), one per `texture` and `sampler` pair the
    /// generated shader declares. This takes handles rather than images
    /// precisely because a compiled graph may bind nothing new: two shaders
    /// over one wall are two entries here and one set of images.
    #[cfg(feature = "shader")]
    pub fn insert_static(
        &mut self,
        key: ShaderKey,
        handles: Vec<Handle<Image>>,
    ) -> Vec<Handle<Image>> {
        self.statics.insert(key, handles.clone());
        handles
    }

    /// The image behind one plane, if some compiled graph has already baked
    /// it — whichever graph that was.
    #[cfg(feature = "shader")]
    pub fn get_image(&self, key: &PlaneImageKey) -> Option<&Handle<Image>> {
        self.images.get(key)
    }

    /// Hand one bound image to `images` and remember it by the plane behind it.
    ///
    /// The main-thread half of a compiled graph's static bake, the way
    /// [`insert`](Self::insert) is of a whole one. Re-inserting a key replaces
    /// the entry; nothing does, because a key that is already here is looked up
    /// rather than rebuilt.
    #[cfg(feature = "shader")]
    pub fn insert_image(
        &mut self,
        key: PlaneImageKey,
        texture: Image,
        images: &mut Assets<Image>,
    ) -> Handle<Image> {
        let handle = images.add(texture);
        self.images.insert(key, handle.clone());
        handle
    }

    /// Record that `planes` planes were rasterised for a compiled graph.
    ///
    /// Called with what a rasterisation actually cost — every plane the
    /// expression holds, the intermediates a filter reads included — and not
    /// called at all when every bound image was already here.
    #[cfg(feature = "shader")]
    pub(crate) fn note_rasterised(&mut self, planes: usize) {
        self.rasterised = self.rasterised.saturating_add(planes);
    }

    /// How many planes have been rasterised for compiled graphs since this
    /// cache was made, or since it was last [`clear`](Self::clear)ed.
    #[cfg(feature = "shader")]
    pub fn rasterised(&self) -> usize {
        self.rasterised
    }

    /// The strands of a scatter this cache has already run, if it has.
    #[cfg(feature = "strand-scatter")]
    pub fn get_strands(
        &self,
        key: &crate::strands::StrandKey,
    ) -> Option<&Arc<ashlar_material::strands::StrandSet>> {
        self.strands.get(key)
    }

    /// Remember one scattered set.
    ///
    /// Re-inserting a key replaces the entry, which is what a re-scatter of an
    /// edited graph wants; whoever still holds the previous set keeps it until
    /// they drop it.
    #[cfg(feature = "strand-scatter")]
    pub fn insert_strands(
        &mut self,
        key: crate::strands::StrandKey,
        set: ashlar_material::strands::StrandSet,
    ) -> Arc<ashlar_material::strands::StrandSet> {
        let set = Arc::new(set);
        self.strands.insert(key, Arc::clone(&set));
        set
    }

    /// How many distinct strand sets are held.
    #[cfg(feature = "strand-scatter")]
    pub fn strand_sets(&self) -> usize {
        self.strands.len()
    }

    /// How many distinct bound images are held, over every compiled graph.
    ///
    /// Fewer than the bindings those graphs declare wherever two of them share
    /// a plane, which is the whole point of the key.
    #[cfg(feature = "shader")]
    pub fn bound_images(&self) -> usize {
        self.images.len()
    }

    /// How many distinct bakes are held.
    pub fn len(&self) -> usize {
        self.sets.len()
    }

    /// Every bake this cache has run, in no particular order.
    ///
    /// What a caller asks when it wants to know *which* sets it is paying for
    /// rather than how many: a block of buildings over one material key at
    /// three values of one parameter is three keys naming one graph, and this
    /// is how a test or a diagnostic says so.
    pub fn keys(&self) -> impl Iterator<Item = &BakeKey> {
        self.sets.keys()
    }

    /// Whether nothing has been baked yet.
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// How many distinct compiled graphs have their bound textures here.
    #[cfg(feature = "shader")]
    pub fn statics(&self) -> usize {
        self.statics.len()
    }

    /// Forget every set.
    ///
    /// The images live until the last handle to them drops, so this is a
    /// release rather than a free. Two callers want it: a gallery moving to a
    /// scene with its own graph library, and a preview that reloaded a library
    /// in place — in both, the same key now means a different graph.
    pub fn clear(&mut self) {
        self.sets.clear();
        #[cfg(feature = "shader")]
        {
            self.statics.clear();
            self.images.clear();
            self.rasterised = 0;
        }
        #[cfg(feature = "strand-scatter")]
        self.strands.clear();
    }
}

/// What [`create_graph_material`] needs beyond an [`AssetServer`], for the surfaces
/// that name a graph.
///
/// A borrow bundle rather than four arguments, because it is passed down two
/// levels and a system that creates materials already holds all four as
/// parameters.
pub struct BakeContext<'a> {
    /// The library a [`ashlar::Surface::Graph`] names into, as
    /// [`read_graphs`](crate::read_graphs) reads it and
    /// [`read_library`](crate::read_library) preflighted it.
    pub graphs: &'a MaterialGraphLibrary,
    /// Where a bake's images are found and remembered.
    pub cache: &'a mut BakeCache,
    /// Where a new bake's images are put.
    pub images: &'a mut Assets<Image>,
    /// How the bake divides its rows. `None` is one thread per core, bounded
    /// by the work; a caller sharing the machine with something else says a
    /// number. The bytes are the same either way.
    pub threads: Option<NonZeroUsize>,
    /// Which backend evaluates the expression. [`Baker::Cpu`] is the
    /// reference and needs nothing; [`Baker::Gpu`] is the same bake through the
    /// same emitter on a device, for a caller that has one and is being looked
    /// at while it bakes.
    pub baker: Baker<'a>,
}

/// Which backend a bake's expression runs on.
///
/// An enum on the context rather than two entry points, because *every* caller
/// of `create_graph_material` and `create_shader_material` has the same question to answer and
/// the same two answers: a preview and a game have a render device and should
/// use it, and a content step, a test and a headless tool do not and must not
/// need one. A caller that has a device says so once, where it builds the
/// context, and nothing below that forks on it.
///
/// [`Cpu`](Self::Cpu) is [`Default`] on purpose. The CPU bake is the reference,
/// it runs anywhere, and the bytes it writes are what the shipped files hold;
/// a path that wants the other one asks for it.
#[derive(Clone, Copy, Debug, Default)]
pub enum Baker<'a> {
    /// The interpreter, across [`BakeContext::threads`] threads.
    #[default]
    Cpu,
    /// A compute dispatch per plane and one for the outputs, on this device.
    #[cfg(feature = "gpu-bake")]
    Gpu(&'a crate::gpu::GpuBaker),
    /// Never constructed, and here so that the lifetime this enum is generic
    /// over is used in every build.
    ///
    /// Without `gpu-bake` the only variant left names no device and so names no
    /// borrow, which makes `Baker<'a>` an unused-parameter error rather than a
    /// CPU-only enum. A `PhantomData` variant is the cheap way to keep one type
    /// across both builds: a caller still writes `Baker::Cpu`, and
    /// [`BakeContext`] still carries one lifetime whether or not a device
    /// exists.
    #[cfg(not(feature = "gpu-bake"))]
    #[doc(hidden)]
    Never(std::marker::PhantomData<&'a ()>),
}

// Both of these name a device, so the whole block is the `gpu-bake` build's.
// Without it `Baker` is the CPU variant and the `PhantomData` that keeps its
// lifetime, and it has nothing to answer.
#[cfg(feature = "gpu-bake")]
impl<'a> Baker<'a> {
    /// A GPU baker where there is one, and the CPU otherwise.
    ///
    /// The shape a caller with an `Option<&GpuBaker>` wants, which is every
    /// caller that reads a render device out of a world that may not have one.
    pub fn or_cpu(gpu: Option<&'a crate::gpu::GpuBaker>) -> Self {
        gpu.map_or(Self::Cpu, Self::Gpu)
    }

    /// The device this bakes on, where it bakes on one.
    pub fn gpu(self) -> Option<&'a crate::gpu::GpuBaker> {
        match self {
            Self::Cpu => None,
            Self::Gpu(gpu) => Some(gpu),
        }
    }
}

/// Bake one surface into images, without touching the ECS.
///
/// The expensive half of the runtime bake path, and a plain function on plain
/// data: it takes a bake and a graph library, rasterises every output and its
/// whole mip chain, and answers the encoded maps as [`Image`]s. Nothing here
/// reads a `World`, an `Assets` or an `AssetServer`, which is what makes it
/// callable from a task:
///
/// ```no_run
/// use ashlar_bevy::runtime_bake::{BakeCache, BakeKey, GraphTextures, bake_images};
/// use ashlar_material::MaterialGraphLibrary;
/// use bevy::prelude::*;
/// use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
///
/// /// A bake in flight. The key travels with it, because the cache is keyed
/// /// by the bake and the task no longer holds one.
/// #[derive(Component)]
/// struct Baking(BakeKey, Task<anyhow::Result<GraphTextures>>);
///
/// fn start(mut commands: Commands, graphs: Res<Graphs>, bake: Res<Wanted>) {
///     // The library is cloned into the task: a graph is plain data, and the
///     // resource cannot be borrowed across a frame.
///     let (library, wanted) = (graphs.0.clone(), bake.0.clone());
///     let key = BakeKey::new(&wanted);
///     let task = AsyncComputeTaskPool::get()
///         .spawn(async move { bake_images(&wanted, &library, None) });
///     commands.spawn(Baking(key, task));
/// }
///
/// /// The main-thread half: one `Assets<Image>` insert per finished bake.
/// fn finish(
///     mut commands: Commands,
///     mut baking: Query<(Entity, &mut Baking)>,
///     mut cache: ResMut<BakeCache>,
///     mut images: ResMut<Assets<Image>>,
/// ) -> Result {
///     for (entity, mut task) in &mut baking {
///         let Some(done) = block_on(future::poll_once(&mut task.1)) else {
///             continue;
///         };
///         cache.insert(task.0.clone(), done?, &mut images);
///         commands.entity(entity).despawn();
///     }
///     Ok(())
/// }
///
/// #[derive(Resource)]
/// struct Wanted(ashlar::Bake);
///
/// /// The game's graph library, as `ashlar_bevy::read_graphs` read it.
/// #[derive(Resource)]
/// struct Graphs(MaterialGraphLibrary);
/// ```
///
/// The bake always asks for mips, because a runtime-baked wall is looked at
/// from the same distances a file-baked one is and Bevy builds no chain for an
/// image it was handed. That costs about a quarter again of level 0's time and
/// a third of its bytes.
pub fn bake_images(
    bake: &ashlar::Bake,
    graphs: &MaterialGraphLibrary,
    threads: Option<NonZeroUsize>,
) -> Result<GraphTextures> {
    let graph = graph_of(&bake.graph, graphs)?;
    let set = ashlar_material::bake::bake(&BakeRequest {
        graph,
        library: graphs,
        params: &bake.params,
        resolution: bake.resolution,
        mips: true,
        threads,
    })
    .with_context(|| format!("baking material graph {:?}", bake.graph))?;
    Ok(GraphTextures::from(&set))
}

/// The images of one bake, from the cache or freshly baked into it.
///
/// Synchronous: the bake runs on the calling thread. That is what a preview, a
/// content step and a test want; a game that bakes while it is being played
/// wants [`bake_images`] in a task and [`BakeCache::insert`] here.
pub fn graph_images(bake: &ashlar::Bake, cx: &mut BakeContext<'_>) -> Result<GraphImages> {
    let key = BakeKey::new(bake);
    if let Some(images) = cx.cache.get(&key) {
        return Ok(images.clone());
    }
    // The device path only where one can exist. Without `gpu-bake` there is no
    // variant that holds a baker, so this is the CPU bake and no branch at all.
    #[cfg(feature = "gpu-bake")]
    let textures = match cx.baker.gpu() {
        Some(gpu) => crate::gpu::bake_images_gpu(bake, cx.graphs, gpu, cx.threads)?,
        None => bake_images(bake, cx.graphs, cx.threads)?,
    };
    #[cfg(not(feature = "gpu-bake"))]
    let textures = bake_images(bake, cx.graphs, cx.threads)?;
    Ok(cx.cache.insert(key, textures, cx.images))
}

/// Create a PBR material for any surface, baking a [`ashlar::Surface::Graph`]
/// one through the image cache.
///
/// The superset of [`create_material`](crate::create_material): `Plain` and `Files` are that
/// function's answer unchanged, `Graph` is baked once per distinct
/// [`ashlar::Bake`] and shared after that, and `Shader` is refused with the
/// same message [`read_library`](crate::read_library) gives it until the WGSL backend lands.
/// A library that may hold a `Graph` surface goes through here and never
/// matches on the variant itself.
///
/// The two entry points are two because their costs are two. `create_material` needs one
/// borrow, cannot fail, and is what a game shipping texture files calls; making
/// it take a graph library, a cache and `Assets<Image>` would push all three
/// onto every caller that never bakes anything, and a `Result` with them. So
/// the simple path stayed simple and the baking path is the one that carries
/// the machinery.
///
/// Height is baked and kept on [`GraphImages`], and reaches no
/// [`StandardMaterial`] slot, exactly as a file `height` map does not:
/// `depth_map` wants black-is-top depth and a parallax scale in the mesh's own
/// units, and neither is a decision a `0..=1` field makes.
pub fn create_graph_material(
    definition: &ashlar::MaterialDefinition,
    server: &AssetServer,
    cx: &mut BakeContext<'_>,
) -> Result<StandardMaterial> {
    match &definition.surface {
        ashlar::Surface::Plain | ashlar::Surface::Files { .. } => {
            Ok(crate::create_material(definition, server))
        }
        ashlar::Surface::Graph(bake) => {
            let images = graph_images(bake, cx)?;
            Ok(standard(
                definition,
                Maps {
                    base_color: Some(images.base_color),
                    normal: Some(images.normal),
                    metallic_roughness: Some(images.orm.clone()),
                    occlusion: Some(images.orm),
                    // Linear HDR, and multiplied by the definition's own
                    // emissive constant, which defaults to zero.
                    emissive: images.emissive,
                },
            ))
        }
        // A compiled graph is not a `StandardMaterial` at all: it is a
        // `StandardMaterial` with a fragment shader over it, which is a
        // different asset type and a different `MeshMaterial3d`.
        // `shader::create_shader_material` is what makes one, and it needs `Assets<Shader>`
        // and the registry beside what this takes.
        ashlar::Surface::Shader { graph, .. } => bail!(
            "a Shader surface over graph {graph:?} compiles to a ProceduralMaterial rather than a \
             StandardMaterial; call shader::create_shader_material for it, or bake it with Surface::Graph"
        ),
    }
}
