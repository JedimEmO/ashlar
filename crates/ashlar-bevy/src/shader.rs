//! Compiling a material graph into a live Bevy material.
//!
//! A [`ashlar::Surface::Shader`] definition names a graph and parameter values
//! and asks for the graph itself rather than a picture of it. This module is
//! what that costs and what it buys.
//!
//! The graph is **partitioned**: everything that does not move is baked once
//! into bound textures, and what is left — the live parameters, the clock, and
//! every instruction downstream of them — is emitted as WGSL and evaluated per
//! fragment. [`ashlar_material::partition`] decides the cut and
//! [`ashlar_material::wgsl`] prints it; this module is the adapter that hands
//! Bevy the pieces:
//!
//! - the static half as `Image` assets, encoded and mipped exactly as the bake
//!   would have encoded the same maps, and kept in the same
//!   [`crate::runtime_bake::BakeCache`] a runtime bake uses;
//! - the live half as two `Shader` assets — a main-pass fragment and a prepass
//!   fragment — registered under ids derived from the graph, so two materials
//!   over one graph share one pipeline;
//! - the live parameter values as one fixed-size uniform block.
//!
//! # How the shader reaches the pipeline
//!
//! [`MaterialExtension::fragment_shader`] is a static function: it cannot vary
//! per graph, and a graph per type is exactly what this design exists to avoid.
//! So [`GraphExtension`] does not use it. It carries a [`GraphKey`] as its
//! [`bind_group_data`](bevy::render::render_resource::AsBindGroup::bind_group_data),
//! which Bevy hands to [`MaterialExtension::specialize`] and — just as
//! importantly — makes part of the pipeline cache key, and `specialize` swaps
//! the descriptor's fragment shader for a handle derived from that key.
//! [`register`] is what put a shader under that handle.
//!
//! Three consequences worth stating, because each is a cost the ADR named:
//!
//! - **A shader per graph is a pipeline per graph**, and the key is exact: two
//!   definitions over one graph with one live set and one set of *folded*
//!   values share a pipeline and differ only in a uniform. Two that fold a
//!   parameter differently do not, because folding a parameter changes the
//!   generated text.
//! - **Registration is not lazy.** [`create_shader_material`] compiles and bakes on the
//!   calling thread, like [`create_graph_material`](crate::runtime_bake::create_graph_material),
//!   because a material that appears three frames late is a wall that was
//!   briefly the wrong colour.
//! - **The handle is a weak one.** Nothing owns the shader asset but the
//!   registry, and the registry is a resource; dropping it drops the shaders,
//!   and any pipeline still holding one keeps working until it is rebuilt.
//!
//! # What is not checked here
//!
//! [`read_library`](crate::read_library) preflights a `Shader` surface by lowering it,
//! partitioning it and emitting both stages, so an unknown graph, a parameter
//! the graph does not declare, a live parameter this backend cannot honour and
//! a graph that binds more textures than a material may are all startup errors.
//! What preflight cannot do is *validate the WGSL*: `naga` is a
//! dev-dependency of `ashlar-material`, deliberately, so that a game linking
//! this crate does not link a shader front end it will never call. The emitted
//! text is validated in that crate's own tests and compiled for real by the
//! first frame that draws the material.
use std::{
    collections::{BTreeMap, HashMap},
    num::NonZeroUsize,
};

use anyhow::{Context, Result, bail, ensure};
use ashlar_material::{
    Material, MaterialGraphLibrary, ParamValue,
    bake::{PlaneFormat, encode_bound},
    interp::Plane,
    ir::{Filter, IrType, Target},
    partition::{BoundTexture, CostReport, MAX_BOUND_TEXTURES, Partition, partition},
    planes::{PlaneKey, plane_keys, rasterise_wanted},
    wgsl::{self, Stage, TextureBinding, UniformLayout},
};
use bevy::{
    asset::uuid::Uuid,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    },
    shader::{Shader, ShaderDefVal},
};

use crate::runtime_bake::{BakeCache, Baker, PlaneImageKey, ShaderKey, graph_of};

/// How many bound textures one compiled graph may have, and so how many
/// texture and sampler pairs [`GraphExtension`] declares.
///
/// The bind group is declared by the type, so the count cannot depend on which
/// graph is drawn: every material of this type has eight pairs and fills the
/// ones its graph bound. The partition refuses a graph that needs more, at
/// exactly this number.
pub const MAX_TEXTURES: usize = MAX_BOUND_TEXTURES;

/// `vec4` rows in the uniform block [`GraphExtension`] declares.
///
/// The block is fixed-size for the same reason the texture count is: one type,
/// one layout, every graph. Sixteen rows is 256 bytes, which holds sixteen
/// colours or sixty-four sliders — far past what a material with a parameter
/// panel in front of it has ever wanted — and costs a quarter of a kilobyte per
/// material either way.
pub const PARAM_ROWS: usize = 16;

/// Bytes in that block.
pub const PARAM_BYTES: usize = PARAM_ROWS * 16;

/// Texels per repeat of a compiled graph's bound textures, when nothing says.
///
/// A [`ashlar::Surface::Shader`] carries no resolution, unlike a
/// [`ashlar::Bake`], because the design's `Shader` variant names a graph and
/// its parameters and nothing about texels. The static half still has to be
/// rasterised at *some* size, and this is it;
/// [`ShaderContext::resolution`] is how a caller says another.
pub const DEFAULT_RESOLUTION: u32 = 1024;

/// The uniform block a compiled graph's live parameters are written into.
///
/// Sixteen `vec4` rows, whatever the graph declares. The
/// [`UniformLayout`] the emitter answers
/// says where each parameter sits inside it; everything past the last one is
/// zero, and the shader's own `AshlarParams` struct — which is only as long as
/// the graph's parameters need — reads the front of it.
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct GraphParams {
    /// The block, as the rows a uniform buffer is laid out in.
    pub rows: [Vec4; PARAM_ROWS],
}

impl Default for GraphParams {
    fn default() -> Self {
        Self {
            rows: [Vec4::ZERO; PARAM_ROWS],
        }
    }
}

impl GraphParams {
    /// A block filled from the front with `bytes`, and zero after them.
    ///
    /// `bytes` is what
    /// [`UniformLayout::bytes`](ashlar_material::wgsl::UniformLayout::bytes)
    /// answers: the graph's own block, laid out the way the generated struct
    /// reads it. Anything past [`PARAM_BYTES`] is dropped, which cannot happen
    /// for a graph [`create_shader_material`] accepted — it refuses a longer block by
    /// path first — and is a truncation rather than a panic if it ever does.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut block = [0_u8; PARAM_BYTES];
        let taken = bytes.len().min(PARAM_BYTES);
        block[..taken].copy_from_slice(&bytes[..taken]);
        let mut rows = [Vec4::ZERO; PARAM_ROWS];
        let (chunks, _) = block.as_chunks::<16>();
        for (row, chunk) in rows.iter_mut().zip(chunks) {
            let lane = |at: usize| {
                let mut four = [0_u8; 4];
                four.copy_from_slice(&chunk[at..at + 4]);
                f32::from_le_bytes(four)
            };
            *row = Vec4::new(lane(0), lane(4), lane(8), lane(12));
        }
        Self { rows }
    }
}

/// What makes two compiled graphs one pipeline: a digest of the
/// [`ShaderKey`] that describes them.
///
/// A digest rather than the description, and that is forced rather than
/// chosen: Bevy packs a material's bind group data into a `repr(C, packed)`
/// struct it copies, so the type has to be [`Copy`], and a `String` is not. The
/// collision that a digest admits is caught rather than lived with —
/// [`register`] holds the full [`ShaderKey`] beside every entry and refuses a
/// second graph that lands on the same sixty-four bits, so the failure is a
/// startup error naming both graphs rather than one building silently wearing
/// another's shader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GraphKey(u64);

impl GraphKey {
    /// The digest of one exact key.
    pub fn new(key: &ShaderKey) -> Self {
        Self(key.digest())
    }

    /// The sixty-four bits themselves, for a log line or a test.
    pub fn bits(self) -> u64 {
        self.0
    }
}

/// The high bits every generated shader id shares.
///
/// A random constant, so that an id derived here cannot collide with Bevy's own
/// embedded shaders or with another crate's. The low seventy-two bits are the
/// [`GraphKey`] and the stage.
const SHADER_NAMESPACE: u128 = 0x0000_b19a_7c4e_5f2d;

/// The UUID one graph's shader for one stage is registered under.
///
/// Pure, and that is the whole mechanism: the render world's `specialize`
/// derives the same id the main world's [`register`] inserted under, without
/// either of them sharing a resource or sending a message.
fn shader_uuid(key: GraphKey, stage: Stage) -> Uuid {
    let stage = match stage {
        Stage::Fragment => 0_u128,
        Stage::Prepass => 1,
    };
    Uuid::from_u128((SHADER_NAMESPACE << 72) | (u128::from(key.bits()) << 8) | stage)
}

/// The asset id that shader is registered under.
pub fn shader_id(key: GraphKey, stage: Stage) -> AssetId<Shader> {
    AssetId::Uuid {
        uuid: shader_uuid(key, stage),
    }
}

/// A weak handle to that shader.
///
/// Weak because nothing here owns the asset: the registry does, and a pipeline
/// built from one keeps working until it is rebuilt.
pub fn shader_handle(key: GraphKey, stage: Stage) -> Handle<Shader> {
    Handle::Uuid(shader_uuid(key, stage), std::marker::PhantomData)
}

/// The live half of a material graph, as a Bevy material extension.
///
/// One type for every graph. The bind group is fixed — a uniform block at 100
/// and eight texture and sampler pairs from 101 — and a graph fills the front
/// of both; [`GraphKey`] is what tells two graphs apart, both to
/// [`specialize`](MaterialExtension::specialize) and to the pipeline cache.
///
/// The bindings are at 100 and up because `StandardMaterial` owns everything
/// below: this extension is added to it, not instead of it, so the generated
/// fragment can call `pbr_input_from_standard_material` and multiply into what
/// it answers.
#[derive(Asset, AsBindGroup, Clone, Debug, Default, TypePath)]
#[bind_group_data(GraphKey)]
pub struct GraphExtension {
    /// The live parameters, laid out as the generated `AshlarParams` reads
    /// them.
    #[uniform(100)]
    pub params: GraphParams,
    /// Bound image 0. A graph that bound fewer leaves the rest `None`, and
    /// Bevy binds its fallback image there; the shader never reads it.
    #[texture(101)]
    #[sampler(102)]
    pub static_0: Option<Handle<Image>>,
    /// Bound image 1.
    #[texture(103)]
    #[sampler(104)]
    pub static_1: Option<Handle<Image>>,
    /// Bound image 2.
    #[texture(105)]
    #[sampler(106)]
    pub static_2: Option<Handle<Image>>,
    /// Bound image 3.
    #[texture(107)]
    #[sampler(108)]
    pub static_3: Option<Handle<Image>>,
    /// Bound image 4.
    #[texture(109)]
    #[sampler(110)]
    pub static_4: Option<Handle<Image>>,
    /// Bound image 5.
    #[texture(111)]
    #[sampler(112)]
    pub static_5: Option<Handle<Image>>,
    /// Bound image 6.
    #[texture(113)]
    #[sampler(114)]
    pub static_6: Option<Handle<Image>>,
    /// Bound image 7.
    #[texture(115)]
    #[sampler(116)]
    pub static_7: Option<Handle<Image>>,
    /// Which graph this is, and so which shader draws it.
    pub graph: GraphKey,
}

impl GraphExtension {
    /// The bound images in binding order, so a caller can fill them from a
    /// slice without naming eight fields.
    pub fn set_textures(&mut self, images: &[Handle<Image>]) {
        let mut slots = [
            &mut self.static_0,
            &mut self.static_1,
            &mut self.static_2,
            &mut self.static_3,
            &mut self.static_4,
            &mut self.static_5,
            &mut self.static_6,
            &mut self.static_7,
        ];
        for (slot, image) in slots.iter_mut().zip(images) {
            **slot = Some(image.clone());
        }
    }

    /// The bound images in binding order.
    pub fn textures(&self) -> [Option<&Handle<Image>>; MAX_TEXTURES] {
        [
            self.static_0.as_ref(),
            self.static_1.as_ref(),
            self.static_2.as_ref(),
            self.static_3.as_ref(),
            self.static_4.as_ref(),
            self.static_5.as_ref(),
            self.static_6.as_ref(),
            self.static_7.as_ref(),
        ]
    }
}

impl From<&GraphExtension> for GraphKey {
    fn from(extension: &GraphExtension) -> Self {
        extension.graph
    }
}

/// The shader-def the prepass pipeline pushes and the main pass does not.
///
/// `specialize` is called for both, with no flag saying which, and the two
/// stages need different generated text — so the discriminator is the one
/// Bevy's own prepass sets before it calls out.
const PREPASS_DEF: &str = "PREPASS_PIPELINE";

/// The shader-def that says a prepass is writing a G-buffer rather than a
/// normal and a motion vector.
///
/// The deferred prepass is the one pipeline a compiled graph must keep its
/// hands off. Bevy pushes it beside [`PREPASS_DEF`] (`bevy_pbr`'s
/// `prepass/mod.rs:511`), and under it the prepass `FragmentOutput` grows
/// `deferred: vec4<u32>` and `deferred_lighting_pass_id: u32` at locations 2
/// and 3 (`prepass/prepass_io.wgsl:91-94`). The prepass
/// [`wgsl::emit`] prints writes neither: it sets `normal` and `motion_vector`
/// and returns, so the two G-buffer fields would go out zero-initialised, and a
/// lighting pass id of zero is a fragment the deferred lighting pass does not
/// light. Swapping there trades a slightly plainer surface for an invisible
/// one.
///
/// It is *not* enough on its own, and that is the trap: the main pass pushes
/// the same def for every material drawn on a camera that merely *has* a
/// `DeferredPrepass` (`bevy_pbr`'s `render/mesh.rs:422` puts the bit in the
/// view key and `render/mesh.rs:3463` prints it), so a forward material beside
/// a deferred one would stop being swapped at all. Both defs together are what
/// name the deferred prepass pipeline, because only `PrepassPipeline` pushes
/// [`PREPASS_DEF`].
const DEFERRED_DEF: &str = "DEFERRED_PREPASS";

/// Whether a specialised stage was given a shader-def by that name.
fn defined(defs: &[ShaderDefVal], wanted: &str) -> bool {
    defs.iter().any(|def| {
        let name = match def {
            ShaderDefVal::Bool(name, _)
            | ShaderDefVal::Int(name, _)
            | ShaderDefVal::UInt(name, _) => name,
        };
        name == wanted
    })
}

/// Point a specialised descriptor at the shader `key`'s graph was registered
/// under, where the generated text is an answer to what the pipeline asked.
///
/// Split out of [`MaterialExtension::specialize`] rather than written inside it
/// because everything decided here is decided from the descriptor alone, and
/// the two arguments this does not take — a `MaterialExtensionPipeline` and a
/// vertex layout — are exactly the two a test cannot build without a render
/// world. The swap itself is then a pure function of a descriptor and a key,
/// and `tests` below pins all four of its answers.
fn swap_fragment(descriptor: &mut RenderPipelineDescriptor, key: GraphKey) {
    // A depth-only prepass and a shadow pass have no fragment stage at all,
    // and nothing this material says belongs in one.
    let Some(fragment) = descriptor.fragment.as_mut() else {
        return;
    };
    let prepass = defined(&fragment.shader_defs, PREPASS_DEF);
    if prepass && defined(&fragment.shader_defs, DEFERRED_DEF) {
        // Leaving the descriptor alone leaves Bevy's own choice in place, which
        // for a `ProceduralMaterial` is `StandardMaterial`'s deferred fragment
        // — `ExtendedMaterial` falls through to the base material for a stage
        // the extension does not override. So a graph drawn on a deferred
        // camera is the base material's constants without the graph's per-texel
        // detail: flat where it should be figured, and visible, which is the
        // half of the trade worth keeping.
        // Once per call site rather than once per pipeline: `specialize` runs
        // for every key the cache has not seen, and a deferred camera would
        // otherwise say this per graph, per view and per mesh layout.
        bevy::utils::once!(tracing::warn!(
            "A compiled material graph is being drawn through the deferred prepass, which the \
             generated prepass shader cannot write: it has no G-buffer to fill. The surface \
             falls back to its plain StandardMaterial there. Give the camera a forward renderer \
             — remove DeferredPrepass, or set the material's opaque_render_method to Forward — \
             to see the graph itself."
        ));
        return;
    }
    let stage = if prepass {
        Stage::Prepass
    } else {
        Stage::Fragment
    };
    fragment.shader = shader_handle(key, stage);
}

impl MaterialExtension for GraphExtension {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        swap_fragment(descriptor, key.bind_group_data);
        Ok(())
    }
}

/// A `StandardMaterial` whose per-texel detail is a compiled graph.
///
/// The constants, the tiling and the lighting are Bevy's; the maps are not maps
/// at all but an expression evaluated per fragment over a handful of baked
/// textures.
pub type ProceduralMaterial = ExtendedMaterial<StandardMaterial, GraphExtension>;

/// One compiled graph: its shaders, its layout, and what it cost.
#[derive(Debug)]
pub struct Registered {
    /// The exact key this entry is for, which is what makes a digest collision
    /// an error rather than a wrong picture.
    pub key: ShaderKey,
    /// The main-pass fragment.
    pub fragment: Handle<Shader>,
    /// The prepass fragment, writing the same normal.
    pub prepass: Handle<Shader>,
    /// Where each live parameter sits in [`GraphParams`].
    pub layout: UniformLayout,
    /// One entry per bound image, in binding order.
    pub bindings: Vec<TextureBinding>,
    /// What the fragment costs, as the partition counted it.
    pub report: CostReport,
}

/// The graphs whose shaders are registered, keyed by the digest a material
/// carries.
///
/// Insert it with `App::init_resource`, or take
/// [`ProceduralMaterialPlugin`], which does.
///
/// Nothing evicts, and nothing should: a shader is a few kilobytes and a
/// pipeline behind it. A *reloaded* library is the one case that has to say so
/// — the keys name graphs rather than describing them, exactly as
/// [`BakeCache`]'s do — and [`clear`](Self::clear) is how.
#[derive(Resource, Default, Debug)]
pub struct GraphShaders {
    entries: HashMap<GraphKey, Registered>,
}

impl GraphShaders {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The compiled graph behind a key, if it has been registered.
    pub fn get(&self, key: GraphKey) -> Option<&Registered> {
        self.entries.get(&key)
    }

    /// How many graphs are compiled.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is compiled yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forget every graph, and drop the shader assets with it.
    ///
    /// What this releases is the claim that a key still means what it meant.
    /// It also has to release the assets, because nothing else can: a shader
    /// registered under an [`AssetId::Uuid`] is held in a hash map rather than
    /// by handle count, so dropping the registry frees nothing and an
    /// authoring session that adds or removes a parameter — which changes the
    /// [`ShaderKey`] and so the id — would strand a fragment and a prepass
    /// pair on every reload.
    ///
    /// A pipeline still built on one does not break: Bevy's `PipelineCache`
    /// requeues every pipeline that named a removed shader
    /// (`bevy_render`'s `pipeline_cache.rs:459`), so a graph that is
    /// registered again under the same id is rebuilt rather than lost, and one
    /// that is not was going to stop being drawn anyway.
    pub fn clear(&mut self, shaders: &mut Assets<Shader>) {
        for key in self.entries.keys() {
            for stage in [Stage::Fragment, Stage::Prepass] {
                shaders.remove(shader_id(*key, stage));
            }
        }
        self.entries.clear();
    }
}

/// Compile one partition into shader assets and remember them.
///
/// Idempotent: a key already registered is left alone and its entry answered,
/// which is what makes [`create_shader_material`] cheap the second time. A key already
/// registered for a *different* graph is the digest collision [`GraphKey`]
/// warns about, and is an error here rather than a wrong picture later.
pub fn register<'a>(
    split: &Partition,
    key: ShaderKey,
    registry: &'a mut GraphShaders,
    shaders: &mut Assets<Shader>,
) -> Result<&'a Registered> {
    let id = GraphKey::new(&key);
    if let Some(entry) = registry.entries.get(&id) {
        ensure!(
            entry.key == key,
            "two material graphs hash to the same shader key {:#018x}: {} and {}. \
             Rename one of them; the key is a digest because Bevy's pipeline key has to be Copy.",
            id.bits(),
            entry.key.describe(),
            key.describe()
        );
        return registry
            .entries
            .get(&id)
            .context("a registered graph is registered");
    }
    let fragment = wgsl::emit(split, Stage::Fragment)
        .map_err(anyhow::Error::from)
        .context("emitting the fragment shader")?;
    let prepass = wgsl::emit(split, Stage::Prepass)
        .map_err(anyhow::Error::from)
        .context("emitting the prepass shader")?;
    ensure!(
        fragment.layout().size() as usize <= PARAM_BYTES,
        "this graph's live parameters need {} bytes of uniform and a compiled material carries \
         {PARAM_BYTES}; bake some of them instead",
        fragment.layout().size()
    );
    let name = key.describe();
    let handles = [Stage::Fragment, Stage::Prepass].map(|stage| shader_handle(id, stage));
    for (stage, text) in [(Stage::Fragment, &fragment), (Stage::Prepass, &prepass)] {
        let label = match stage {
            Stage::Fragment => "fragment",
            Stage::Prepass => "prepass",
        };
        shaders
            .insert(
                shader_id(id, stage),
                Shader::from_wgsl(
                    text.module().to_owned(),
                    format!("ashlar/{name}/{label}.wgsl"),
                ),
            )
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| format!("registering the {label} shader for {name}"))?;
    }
    let [fragment_handle, prepass_handle] = handles;
    registry.entries.insert(
        id,
        Registered {
            key,
            fragment: fragment_handle,
            prepass: prepass_handle,
            layout: fragment.layout().clone(),
            bindings: fragment.bindings().to_vec(),
            report: split.report().clone(),
        },
    );
    registry
        .entries
        .get(&id)
        .context("a graph just registered is registered")
}

/// What [`create_shader_material`] needs beyond the definition.
///
/// The borrow bundle [`BakeContext`](crate::runtime_bake::BakeContext) is, plus
/// the two things a compiled graph has that a baked one does not: somewhere to
/// put a shader, and the registry that says which graphs already have one.
pub struct ShaderContext<'a> {
    /// The library a [`ashlar::Surface::Shader`] names into.
    pub graphs: &'a MaterialGraphLibrary,
    /// Where the static half's images are found and remembered.
    pub cache: &'a mut BakeCache,
    /// Where a new bound texture is put.
    pub images: &'a mut Assets<Image>,
    /// Where a newly compiled shader is put.
    pub shaders: &'a mut Assets<Shader>,
    /// Which graphs are already compiled.
    pub registry: &'a mut GraphShaders,
    /// Texels per repeat for the bound textures. `None` is
    /// [`DEFAULT_RESOLUTION`].
    pub resolution: Option<u32>,
    /// How the static bake divides its rows. `None` is one thread per core.
    pub threads: Option<NonZeroUsize>,
    /// Which backend rasterises the static half.
    ///
    /// The same choice [`BakeContext::baker`](crate::runtime_bake::BakeContext)
    /// carries, and the one that decides what compiling a graph costs in front
    /// of a frame: a fully static graph's whole picture is its planes, so this
    /// is the difference between a second and a few milliseconds.
    pub baker: Baker<'a>,
}

impl ShaderContext<'_> {
    /// The resolution this context bakes bound textures at.
    fn texels(&self) -> u32 {
        self.resolution.unwrap_or(DEFAULT_RESOLUTION)
    }
}

/// Compile a [`ashlar::Surface::Shader`] definition into a live material.
///
/// The `Graph` path's twin: [`create_graph_material`](crate::runtime_bake::create_graph_material)
/// bakes a graph into five maps and hands them to a `StandardMaterial`, and
/// this partitions the same graph, bakes the half that cannot move, compiles
/// the half that can, and hands both to a [`ProceduralMaterial`]. The
/// constants, `tile_metres` and `uv_offset` go through `uv_transform` exactly
/// as they do there, so a wall does not change how it tiles when its surface
/// moves between a bake and a shader.
///
/// Synchronous, and that is the same trade
/// [`create_graph_material`](crate::runtime_bake::create_graph_material) makes: the static
/// half is a CPU bake of up to eight planes and costs what a bake costs.
///
/// A surface that is not a `Shader` is an error rather than a flat material,
/// because a caller that reached here has a graph library and a registry in its
/// hands and meant to use them; `create_graph_material` is the entry point that takes
/// every variant.
pub fn create_shader_material(
    definition: &ashlar::MaterialDefinition,
    cx: &mut ShaderContext<'_>,
) -> Result<ProceduralMaterial> {
    let ashlar::Surface::Shader { graph, params } = &definition.surface else {
        bail!(
            "create_shader_material takes a Surface::Shader; this one is not, and create or create_graph_material \
             is what makes it"
        );
    };
    let Compiled { material, split } = compile(graph, params, cx.graphs, cx.texels())
        .with_context(|| format!("material graph {graph:?}"))?;
    let key = ShaderKey::new(graph, cx.texels(), &material, &split);
    let images = static_images(&split, &key, cx)?;
    let values = live_values(&material, &split);
    let entry = register(&split, key, cx.registry, cx.shaders)?;
    let mut extension = GraphExtension {
        params: GraphParams::from_bytes(&entry.layout.bytes(&values)),
        graph: GraphKey::new(&entry.key),
        ..GraphExtension::default()
    };
    extension.set_textures(&images);
    Ok(ProceduralMaterial {
        base: crate::standard(definition, crate::Maps::default()),
        extension,
    })
}

/// A graph with a definition's values bound into it, and the split of it.
///
/// The two travel together because neither is enough on its own: the partition
/// says which parameters stayed live, and only the material knows what they are
/// worth.
pub(crate) struct Compiled {
    /// The graph with the definition's parameter values in place.
    pub material: Material,
    /// Its shader partition, at the context's resolution.
    pub split: Partition,
}

/// Lower, bind the definition's parameter values, and split.
///
/// Shared by [`create_shader_material`] and [`preflight_shader`], which is the point: a
/// surface that compiles at startup compiles when it is created, and a surface
/// that does not is a startup error.
pub(crate) fn compile(
    graph: &str,
    params: &BTreeMap<String, ashlar::ParamValue>,
    graphs: &MaterialGraphLibrary,
    resolution: u32,
) -> Result<Compiled> {
    let found = graph_of(graph, graphs)?;
    let bound = found.with_params(params)?;
    let material = bound.build_in(graphs)?;
    let target = Target::shader_for(&material);
    let split = partition(&material, &target, resolution)?;
    Ok(Compiled { material, split })
}

/// One parameter value as the three lanes both backends read it in.
///
/// The convention [`Inputs::params`](ashlar_material::interp::Inputs) uses — a
/// scalar in the first lane and a colour in all three — because the interpreter
/// and the generated shader have to read one number the same way.
#[must_use]
pub fn lanes(value: ParamValue) -> [f32; 3] {
    match value {
        ParamValue::Color(rgb) => rgb,
        ParamValue::Float(value) => [value, 0.0, 0.0],
        #[expect(
            clippy::cast_precision_loss,
            reason = "an authored count, not a bit pattern; the IR runs on f32"
        )]
        ParamValue::Int(value) => [value as f32, 0.0, 0.0],
        ParamValue::Bool(value) => [f32::from(u8::from(value)), 0.0, 0.0],
    }
}

/// One filled uniform block: every live parameter the layout declares, taken
/// from `values` by name.
///
/// This is what lets a live parameter move without a rebake and without a
/// recompile. A compiled material's parameters are a uniform, the layout says
/// where each one sits in it, and writing a new block over the old one is the
/// whole of the update — no texture is re-rasterised, no shader is re-emitted,
/// and no pipeline is rebuilt, because none of them depended on the value. A
/// caller that wants a *different* value for a parameter the graph folded has
/// to compile a different graph; [`GraphKey`] is what says which.
///
/// A name the map does not hold reads as zero rather than as an error: a
/// parameter missing from a caller's map is one slider that does nothing, and a
/// process that stops is worse.
#[must_use]
pub fn block(layout: &UniformLayout, values: &BTreeMap<String, ParamValue>) -> GraphParams {
    let lanes = layout
        .entries()
        .iter()
        .map(|entry| values.get(&entry.name).copied().map_or([0.0; 3], lanes))
        .collect::<Vec<_>>();
    GraphParams::from_bytes(&layout.bytes(&lanes))
}

/// Every live parameter's value, three lanes each, in the order the uniform
/// block declares them.
fn live_values(material: &Material, split: &Partition) -> Vec<[f32; 3]> {
    split
        .runtime()
        .params()
        .iter()
        .map(|binding| {
            // A binding the graph does not declare cannot happen: the partition
            // built the list out of the graph's own parameters. Zero rather
            // than a panic if it ever does, because an unlit slider is a better
            // failure than a dead process.
            material
                .param(&binding.name)
                .map_or([0.0; 3], |param| lanes(param.value))
        })
        .collect()
}

/// The static half as image handles in binding order, each baked once per
/// distinct plane rather than once per compiled graph.
///
/// Two lookups. The [`ShaderKey`] one answers a graph that has been compiled
/// before — the same definition in a second scene, or a second instance of
/// it — and costs nothing. The [`PlaneImageKey`] one answers an *image* that
/// some other graph has already baked, which is how `study:concrete-wet` and
/// `study:concrete-cut-aware` share the `study:concrete` wall they both
/// instance instead of each rasterising it.
///
/// The keys are computed before anything is rasterised, which is the whole
/// trick: [`plane_keys`] walks the expression rather than evaluating it, so a
/// graph whose every bound image is already here never reaches a backend at
/// all. The two graphs above share five of their seven images and have two of
/// their own each — a cut-aware wall dulls its sawn faces and a wet one pools
/// water in its own hollows — so the ask is per plane rather than
/// all-or-nothing: only the missing planes and the planes those read are
/// rasterised, on whichever backend, through the same
/// [`plane_closure`](ashlar_material::planes::plane_closure) either way. A
/// plane is a plane whichever device drew it, to within the budget
/// `just conformance` holds them to.
fn static_images(
    split: &Partition,
    key: &ShaderKey,
    cx: &mut ShaderContext<'_>,
) -> Result<Vec<Handle<Image>>> {
    if let Some(images) = cx.cache.get_static(key) {
        return Ok(images.to_vec());
    }
    let bindings = wgsl::emit(split, Stage::Fragment)
        .map_err(anyhow::Error::from)
        .context("laying out the bindings of a compiled graph")?;
    let keys = plane_keys(split.runtime(), split.resolution());
    let wanted = bindings
        .bindings()
        .iter()
        .map(|binding| image_key(binding, split.textures(), &keys))
        .collect::<Result<Vec<_>>>()?;
    // Which planes are still missing, as a flag per plan. A graph whose every
    // bound image is already here asks for nothing and rasterises nothing.
    let mut missing = vec![false; split.textures().len()];
    for (binding, wanted) in bindings.bindings().iter().zip(&wanted) {
        if cx.cache.get_image(wanted).is_some() {
            continue;
        }
        for buffer in &binding.buffers {
            if let Some(slot) = missing.get_mut(buffer.index()) {
                *slot = true;
            }
        }
    }
    let planes = if missing.contains(&true) {
        // The CPU rasterisation, as a closure, because it is what the
        // `gpu-bake` build falls back to and what a build without that feature
        // does unconditionally — and writing it twice is how the two drift.
        let on_the_cpu = || {
            rasterise_wanted(
                split.runtime(),
                split.resolution(),
                &missing,
                cx.threads,
                &mut ashlar_material::planes::BakeCache::new(),
            )
            .map_err(anyhow::Error::from)
            .context("rasterising the static half of a compiled graph")
        };
        #[cfg(feature = "gpu-bake")]
        let planes = match cx.baker.gpu() {
            // A static half the device cannot split goes to the CPU, which is
            // the reference; see `gpu::dispatchable`.
            Some(gpu) if crate::gpu::dispatchable(split.runtime(), split.resolution()) => gpu
                .planes_wanted(split.runtime(), split.resolution(), &missing, cx.threads)
                .context("rasterising the static half of a compiled graph on the GPU")?,
            _ => on_the_cpu()?,
        };
        #[cfg(not(feature = "gpu-bake"))]
        let planes = on_the_cpu()?;
        cx.cache.note_rasterised(planes.iter().flatten().count());
        planes
    } else {
        Vec::new()
    };
    let mut built = Vec::with_capacity(wanted.len());
    for (binding, wanted) in bindings.bindings().iter().zip(wanted) {
        if let Some(handle) = cx.cache.get_image(&wanted) {
            built.push(handle.clone());
            continue;
        }
        // Every plane this binding reads was asked for above, so the only way
        // one is missing here is a plan list that does not match its own
        // bindings; `bound_image` says which by name rather than panicking.
        let image = bound_image(binding, split.textures(), &planes)?;
        built.push(cx.cache.insert_image(wanted, image, cx.images));
    }
    Ok(cx.cache.insert_static(key.clone(), built))
}

/// The key of one bound image: its format, and the plane in each of its lanes.
///
/// The lane matters because it is where the plane's channels were written, so
/// two images packing the same planes into different channels are two images.
/// Everything else the encoder depends on — the resolution, the value type and
/// the filter, a `Filter::Normal`'s half-and-half encoding included — is inside
/// the [`PlaneKey`] already.
fn image_key(
    binding: &TextureBinding,
    textures: &[BoundTexture],
    keys: &[PlaneKey],
) -> Result<PlaneImageKey> {
    let mut lanes = Vec::with_capacity(binding.buffers.len());
    for buffer in &binding.buffers {
        let texture = textures
            .get(buffer.index())
            .with_context(|| format!("bound texture {buffer} has no plan"))?;
        let plane = keys
            .get(buffer.index())
            .with_context(|| format!("bound texture {buffer} names no plane"))?;
        lanes.push((texture.lane, plane.clone()));
    }
    Ok(PlaneImageKey::new(binding.format, lanes))
}

/// One bound image, from the planes the partition cut into it.
///
/// An image is not a plane: a graph whose scalar cuts were packed four to an
/// RGBA image has one image and four planes, each writing the channel its
/// binding owns. The unpacked case is one plane writing the channels its type
/// has, which is the ordinary one.
fn bound_image(
    binding: &TextureBinding,
    textures: &[BoundTexture],
    planes: &[Option<Plane>],
) -> Result<Image> {
    let mut resolution = 0;
    let mut lanes: Vec<[f32; 4]> = Vec::new();
    let mut normal = false;
    for buffer in &binding.buffers {
        let texture = textures
            .get(buffer.index())
            .with_context(|| format!("bound texture {buffer} has no plan"))?;
        let plane = planes
            .get(buffer.index())
            .and_then(Option::as_ref)
            .with_context(|| format!("bound texture {buffer} was never rasterised"))?;
        if lanes.is_empty() {
            resolution = plane.resolution();
            // Opaque, as every map a bake writes is.
            lanes = vec![[0.0, 0.0, 0.0, 1.0]; plane.texels()];
        }
        ensure!(
            plane.resolution() == resolution,
            "bound image {} packs planes of {resolution} and {} texels",
            binding.image,
            plane.resolution()
        );
        // A `Filter::Normal` plane runs -1..=1 and every unorm format holds
        // 0..=1, so the encoder writes it half and half about zero — which is
        // exactly what the generated shader undoes for this one case.
        normal |= matches!(texture.filter, Filter::Normal { .. });
        let width = match texture.value_type {
            IrType::Float => 1,
            IrType::Vec2 => 2,
            IrType::Vec3 => 3,
        };
        let first = usize::from(texture.lane);
        for (index, texel) in lanes.iter_mut().enumerate() {
            let value = plane.texel_at(index);
            for (lane, component) in value.iter().enumerate().take(width) {
                if let Some(slot) = texel.get_mut(first + lane) {
                    *slot = *component;
                }
            }
        }
    }
    ensure!(
        !lanes.is_empty(),
        "bound image {} holds no plane",
        binding.image
    );
    let encode = normal
        && matches!(
            binding.format,
            PlaneFormat::Rgba8Unorm | PlaneFormat::Rgba8Srgb | PlaneFormat::R16Unorm
        );
    let encoded = encode_bound(&lanes, resolution, binding.format, encode);
    Ok(crate::runtime_bake::image(&encoded, resolution))
}

/// Preflight one `Shader` surface: lower it, split it and print both stages.
///
/// Everything a compiled material can be refused for except a WGSL error, and
/// that exception is deliberate — see the module documentation. What this does
/// catch is the whole of the rest: an unknown graph, a parameter the graph does
/// not declare or gives the wrong type, a free field, a node this backend
/// cannot lower, a runtime input above a filter only a plane can run, a graph
/// that binds more than [`MAX_TEXTURES`] images, and a live set whose uniform
/// block is longer than [`PARAM_BYTES`].
pub(crate) fn preflight_shader(
    graph: &str,
    params: &BTreeMap<String, ashlar::ParamValue>,
    graphs: &MaterialGraphLibrary,
) -> Result<()> {
    let Compiled { split, .. } = compile(graph, params, graphs, DEFAULT_RESOLUTION)
        .with_context(|| format!("material graph {graph:?}"))?;
    for stage in [Stage::Fragment, Stage::Prepass] {
        let shader = wgsl::emit(&split, stage)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("material graph {graph:?}"))?;
        ensure!(
            shader.layout().size() as usize <= PARAM_BYTES,
            "material graph {graph:?}: its live parameters need {} bytes of uniform and a \
             compiled material carries {PARAM_BYTES}",
            shader.layout().size()
        );
    }
    Ok(())
}

/// [`MaterialPlugin`] for [`ProceduralMaterial`], with the two resources
/// [`create_shader_material`] needs.
///
/// Add it beside `DefaultPlugins` wherever a library may hold a
/// [`ashlar::Surface::Shader`]. It brings a [`BakeCache`] with it, which a game
/// that also bakes already has and which `init_resource` leaves alone.
#[derive(Default)]
pub struct ProceduralMaterialPlugin;

impl Plugin for ProceduralMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<ProceduralMaterial>::default())
            .init_resource::<GraphShaders>()
            .init_resource::<BakeCache>();
    }
}

#[cfg(test)]
mod tests {
    use bevy::render::render_resource::FragmentState;

    use super::*;

    /// A key that is nobody's graph, which is all these cases need: the swap is
    /// a function of the key and the defs, and never of what was registered.
    const KEY: GraphKey = GraphKey(0x5ea7_1ed5_0f00);

    /// The handle a descriptor arrives with, standing in for whatever Bevy
    /// chose — `pbr.wgsl` in the main pass, `prepass.wgsl` or the base
    /// material's deferred fragment in a prepass. A case that must *not* swap
    /// is a case that still holds this.
    fn untouched() -> Handle<Shader> {
        Handle::Uuid(Uuid::from_u128(0xbe_11), std::marker::PhantomData)
    }

    /// A descriptor with a fragment stage carrying `defs`.
    fn descriptor(defs: &[&str]) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            fragment: Some(FragmentState {
                shader: untouched(),
                shader_defs: defs
                    .iter()
                    .map(|def| ShaderDefVal::Bool((*def).to_owned(), true))
                    .collect(),
                ..default()
            }),
            ..default()
        }
    }

    /// The shader the fragment stage ended up with.
    fn chosen(descriptor: &RenderPipelineDescriptor) -> Handle<Shader> {
        descriptor
            .fragment
            .as_ref()
            .expect("this case has a fragment stage")
            .shader
            .clone()
    }

    #[test]
    fn the_main_pass_takes_the_fragment_stage() {
        let mut descriptor = descriptor(&[]);
        swap_fragment(&mut descriptor, KEY);
        assert_eq!(chosen(&descriptor), shader_handle(KEY, Stage::Fragment));
    }

    #[test]
    fn a_prepass_takes_the_prepass_stage() {
        let mut descriptor = descriptor(&[PREPASS_DEF]);
        swap_fragment(&mut descriptor, KEY);
        assert_eq!(chosen(&descriptor), shader_handle(KEY, Stage::Prepass));
    }

    #[test]
    fn the_deferred_prepass_is_left_to_bevy() {
        let mut descriptor = descriptor(&[PREPASS_DEF, DEFERRED_DEF]);
        swap_fragment(&mut descriptor, KEY);
        // Not the prepass stage, and not the fragment stage either: the
        // generated prepass writes no G-buffer, so the shader Bevy chose —
        // `StandardMaterial`'s deferred fragment — is the one that stays.
        assert_eq!(chosen(&descriptor), untouched());
    }

    #[test]
    fn a_forward_pass_beside_a_deferred_camera_is_still_swapped() {
        // The main pass pushes `DEFERRED_PREPASS` for every material on a
        // camera that has a `DeferredPrepass`, whether or not that material is
        // deferred. Refusing on that def alone would stop swapping a forward
        // material the moment anything else on the camera went deferred.
        let mut descriptor = descriptor(&[DEFERRED_DEF]);
        swap_fragment(&mut descriptor, KEY);
        assert_eq!(chosen(&descriptor), shader_handle(KEY, Stage::Fragment));
    }

    #[test]
    fn a_stage_less_pipeline_is_left_alone() {
        // A depth-only prepass and a shadow pass, which have no fragment stage
        // to swap and nothing for this material to say.
        let mut descriptor = RenderPipelineDescriptor::default();
        swap_fragment(&mut descriptor, KEY);
        assert!(descriptor.fragment.is_none());
    }
}
