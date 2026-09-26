//! Compiling a graph into a `ProceduralMaterial`, without a GPU.
//!
//! Everything below the pipeline: the partition runs, the static half is baked
//! into images with the formats and the chains the generated bindings declare,
//! the live half is registered as two shader assets under ids derived from the
//! graph, and the material carries the key that will pick them. What a headless
//! app cannot check is the pipeline itself — that a specialised descriptor
//! really takes the swapped shader, and that the shader really compiles — which
//! is what the windowed preview is for.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "exact analytic fixtures; a failure is a test failure"
)]
use std::{collections::BTreeMap, num::NonZeroUsize, path::Path};

use ashlar::{Building, Element, Geometry, Instance, MaterialDefinition, Part, Surface};
use ashlar_bevy::{
    runtime_bake::{BakeCache, Baker, ShaderKey, texture_format},
    shader::{
        DEFAULT_RESOLUTION, GraphExtension, GraphKey, GraphParams, GraphShaders, PARAM_BYTES,
        PARAM_ROWS, ProceduralMaterial, ShaderContext, create_shader_material, shader_id,
    },
};
use ashlar_material::{
    Exposure, MaterialGraph, MaterialGraphLibrary, Param, PbrOutput,
    bake::{BakeRequest, PlaneFormat},
    ir::Target,
    mips::levels,
    nodes::{Blend, BlendMode, Math, MathOp, Noise, Subgraph, SurfaceOutput, Time},
    partition::{Partition, partition},
    wgsl::Stage,
};
use bevy::{
    MinimalPlugins,
    asset::AssetPlugin,
    prelude::*,
    render::render_resource::{AsBindGroup, ShaderSize},
    shader::Shader,
};
use tempfile::TempDir;

/// Rows the static bake divides across. Explicit and small: this binary already
/// runs its cases across every core, and a thirty-two-way bake on this machine
/// has been seen to fall over.
const THREADS: Option<NonZeroUsize> = NonZeroUsize::new(8);

/// The smallest resolution a bake takes, which is all a test needs and an
/// eighth of the texels 1024 would be.
const TEXELS: u32 = 256;

/// A graph with one live parameter and a clock: the light strip of the design,
/// in miniature.
///
/// `glow` is `Exposure::Live`, so the partition keeps it as a uniform whatever
/// the target says, and `Time` makes the emissive move — which is the whole
/// point of a compiled material and the thing a bake cannot do.
fn strip() -> MaterialGraph {
    MaterialGraph::builder("study:strip")
        .param(Param::float("glow", 0.6).range(0.0, 4.0).live())
        .param(Param::color("tint", [0.62, 0.82, 0.84]))
        .node("grain", Noise::value().period(8))
        .node("clock", Time::new())
        .node("pulse", Math::new(MathOp::Sin, "clock", 0.0))
        .node("lit", Math::new(MathOp::Mul, "grain", "pulse"))
        .node(
            "emit",
            Math::new(MathOp::Mul, "lit", ashlar_material::Input::param("glow")),
        )
        .node(
            "surface",
            Blend::new(
                BlendMode::Multiply,
                "grain",
                ashlar_material::Input::param("tint"),
            ),
        )
        .output(
            PbrOutput::new()
                .base_color("surface")
                .roughness("grain")
                .height("grain")
                .emissive("emit")
                .normal_strength(0.01),
        )
        .into_graph()
}

/// A graph with nothing live: the partition says to bake it, and the material
/// is a handful of texture reads.
fn still() -> MaterialGraph {
    MaterialGraph::builder("study:still")
        .param(Param::float("wear", 0.25).range(0.0, 1.0))
        .node("grain", Noise::value().period(16))
        .node(
            "worn",
            Math::new(MathOp::Mul, "grain", ashlar_material::Input::param("wear")),
        )
        .output(
            PbrOutput::new()
                .base_color("worn")
                .roughness("grain")
                .height("grain")
                .normal_strength(0.01),
        )
        .into_graph()
}

/// A graph that binds more scalars than a bind group holds, so the partition
/// packs them four to an image — and two of them are PBR outputs, which left to
/// themselves would each ask for their own format.
///
/// Eight interior cuts (a static noise under a live multiply, eight times over),
/// a static roughness and a static height: ten scalar bindings.
fn packed() -> MaterialGraph {
    let mut builder = MaterialGraph::builder("study:packed")
        .param(Param::float("wear", 0.5).range(0.0, 1.0).live());
    let mut total: Option<String> = None;
    for index in 0..8_u32 {
        let noise = format!("n{index}");
        let worn = format!("w{index}");
        builder = builder
            .node(&noise, Noise::value().period(4).seed(index))
            .node(
                &worn,
                Math::new(
                    MathOp::Mul,
                    noise.as_str(),
                    ashlar_material::Input::param("wear"),
                ),
            );
        total = Some(match total {
            None => worn,
            Some(previous) => {
                let id = format!("s{index}");
                builder = builder.node(
                    &id,
                    Math::new(MathOp::Add, previous.as_str(), worn.as_str()),
                );
                id
            }
        });
    }
    builder
        .node("rough", Noise::value().period(4).seed(100))
        .node("tall", Noise::value().period(4).seed(101))
        .output(
            PbrOutput::new()
                .occlusion(total.unwrap().as_str())
                .roughness("rough")
                .height("tall")
                .normal_strength(0.01),
        )
        .into_graph()
}

/// A graph that instances [`still`] and puts a live half over it: the shape of
/// `study:concrete-wet` and `study:concrete-cut-aware`, which is the pair this
/// fixture is here for.
///
/// `wear` is what the wall folds, so binding it through the `Subgraph` is how
/// two of these come to be two *different* walls rather than one shared one.
/// Everything else — the key, and what the live half does with the base
/// colour — changes the fragment and not one texel of the planes.
fn over_still(key: &str, wear: f32, live: MathOp) -> MaterialGraph {
    let wall = |output| {
        Subgraph::new("study:still")
            .param("wear", ashlar_material::ParamValue::Float(wear))
            .output(output)
    };
    MaterialGraph::builder(key)
        .param(Param::float("weather", 0.5).range(0.0, 1.0).live())
        .node("albedo", wall(SurfaceOutput::BaseColor))
        .node("rough", wall(SurfaceOutput::Roughness))
        .node("tall", wall(SurfaceOutput::Height))
        .node(
            "weathered",
            Math::new(
                MathOp::Mul,
                "albedo",
                ashlar_material::Input::param("weather"),
            ),
        )
        .node("face", Math::new(live, "albedo", "weathered"))
        .output(
            PbrOutput::new()
                .base_color("face")
                .roughness("rough")
                .height("tall")
                .normal_strength(0.01),
        )
        .into_graph()
}

fn graphs() -> MaterialGraphLibrary {
    let mut library = MaterialGraphLibrary::default();
    library.insert(strip());
    library.insert(still());
    library.insert(packed());
    // Two graphs over one wall, and a third that folds the wall's own
    // parameter to a different value.
    library.insert(over_still("study:still-wet", 0.25, MathOp::Add));
    library.insert(over_still("study:still-lit", 0.25, MathOp::Sub));
    library.insert(over_still("study:still-worn", 0.9, MathOp::Add));
    library
}

/// A `Shader` surface over `graph`, with `params` bound.
fn definition(graph: &str, params: &[(&str, ashlar::ParamValue)]) -> MaterialDefinition {
    MaterialDefinition {
        tile_metres: [2.4, 1.2],
        uv_offset: [0.25, 0.0],
        emissive: [2.0, 3.5, 4.0],
        surface: Surface::Shader {
            graph: graph.to_owned(),
            params: params
                .iter()
                .map(|(name, value)| ((*name).to_owned(), *value))
                .collect(),
        },
        ..MaterialDefinition::default()
    }
}

/// Everything `create_shader_material` needs, held in one place so a case is one call.
struct Harness {
    app: App,
    graphs: MaterialGraphLibrary,
    cache: BakeCache,
    registry: GraphShaders,
}

impl Harness {
    fn new() -> Self {
        Self::over(graphs())
    }

    /// The same harness over another library, for the one case that wants a
    /// shipped graph rather than a fixture.
    fn over(graphs: MaterialGraphLibrary) -> Self {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<Shader>();
        Self {
            app,
            graphs,
            cache: BakeCache::new(),
            registry: GraphShaders::new(),
        }
    }

    /// The bound image behind each PBR port, by port name.
    ///
    /// A binding knows which planes it holds and a plane knows which output it
    /// *is*, so this is the two joined: `normal` names the image the chain's
    /// coherence rides in, and `roughness` the one the fragment widens.
    fn ports(
        &self,
        split: &ashlar_material::partition::Partition,
        material: &ProceduralMaterial,
    ) -> BTreeMap<String, Handle<Image>> {
        let bindings = self
            .registry
            .get(material.extension.graph)
            .expect("a compiled material is registered")
            .bindings
            .clone();
        let slots = material.extension.textures();
        let mut by_port = BTreeMap::new();
        for (index, binding) in bindings.iter().enumerate() {
            let bound = &split.textures()[binding.buffers[0].index()];
            if let (Some(port), Some(handle)) = (bound.port.as_ref(), slots[index]) {
                by_port.insert(port.clone(), handle.clone());
            }
        }
        by_port
    }

    fn create(&mut self, definition: &MaterialDefinition) -> ProceduralMaterial {
        self.try_create(definition).expect("a preflighted graph")
    }

    fn create_at(&mut self, definition: &MaterialDefinition, texels: u32) -> ProceduralMaterial {
        let (graphs, cache, registry) = (&self.graphs, &mut self.cache, &mut self.registry);
        self.app
            .world_mut()
            .resource_scope(|world, mut images: Mut<Assets<Image>>| {
                world.resource_scope(|_, mut shaders: Mut<Assets<Shader>>| {
                    create_shader_material(
                        definition,
                        &mut ShaderContext {
                            graphs,
                            cache,
                            images: &mut images,
                            shaders: &mut shaders,
                            registry,
                            resolution: Some(texels),
                            threads: THREADS,
                            baker: Baker::Cpu,
                        },
                    )
                })
            })
            .expect("a preflighted graph")
    }

    fn try_create(
        &mut self,
        definition: &MaterialDefinition,
    ) -> anyhow::Result<ProceduralMaterial> {
        let (graphs, cache, registry) = (&self.graphs, &mut self.cache, &mut self.registry);
        self.app
            .world_mut()
            .resource_scope(|world, mut images: Mut<Assets<Image>>| {
                world.resource_scope(|_, mut shaders: Mut<Assets<Shader>>| {
                    create_shader_material(
                        definition,
                        &mut ShaderContext {
                            graphs,
                            cache,
                            images: &mut images,
                            shaders: &mut shaders,
                            registry,
                            resolution: Some(TEXELS),
                            threads: THREADS,
                            baker: Baker::Cpu,
                        },
                    )
                })
            })
    }

    fn image(&self, handle: &Handle<Image>) -> &Image {
        self.app
            .world()
            .resource::<Assets<Image>>()
            .get(handle)
            .expect("a bound texture is an asset")
    }

    fn shader(&self, id: AssetId<Shader>) -> Option<&Shader> {
        self.app.world().resource::<Assets<Shader>>().get(id)
    }
}

fn building() -> Building {
    let part = Part::builder("study:wall")
        .element(Element::new(
            "panel",
            Geometry::cuboid([2.0, 3.0, 0.2]),
            "outer",
        ))
        .build()
        .expect("part");
    Building::builder("study:screen")
        .part(part)
        .material("outer", "paint")
        .instance(Instance::new("only", "study:wall"))
        .build()
        .expect("building")
}

/// Write a one-material library whose surface is `surface`, and read it.
fn read_error(root: &Path, surface: &str) -> String {
    let path = root.join("materials.ron");
    std::fs::write(
        &path,
        format!("(materials: {{\"paint\": (surface: {surface})}})"),
    )
    .expect("fixture library");
    format!(
        "{:#}",
        ashlar_bevy::read_library_with_graphs(&path, root, &building(), &graphs())
            .expect_err("preflight rejects this library")
    )
}

#[test]
fn create_shader_yields_bind_group_data_carrying_the_graph_key() {
    let mut harness = Harness::new();
    let material = harness.create(&definition("study:strip", &[]));
    // What Bevy will hand `specialize`, and what it will put in the pipeline
    // cache key. If this were not the graph's own key, every graph would draw
    // with whichever shader was specialised first.
    let data = material.extension.bind_group_data();
    assert_eq!(data, material.extension.graph);
    let entry = harness
        .registry
        .get(data)
        .expect("creating a material registers its graph");
    assert_eq!(GraphKey::new(&entry.key), data);
    assert_eq!(entry.key.graph(), "study:strip");
    assert_eq!(entry.key.resolution(), TEXELS);
    assert_eq!(entry.key.live(), ["glow"]);
    // A live parameter and a clock: the fragment does real work.
    assert!(entry.report.ops > 0, "{}", entry.report);
    assert!(entry.report.textures > 0, "{}", entry.report);
}

#[test]
fn the_static_images_carry_the_formats_the_bindings_declare() {
    let mut harness = Harness::new();
    let material = harness.create(&definition("study:strip", &[]));
    let key = material.extension.graph;
    let bindings = harness
        .registry
        .get(key)
        .expect("registered")
        .bindings
        .clone();
    assert!(!bindings.is_empty());
    let slots = material.extension.textures();
    for (index, binding) in bindings.iter().enumerate() {
        let handle = slots[index].expect("a bound image fills its slot");
        let image = harness.image(handle);
        assert_eq!(
            image.texture_descriptor.format,
            texture_format(binding.format),
            "image {} is not the format its binding declares",
            binding.image
        );
        assert_eq!(image.texture_descriptor.size.width, TEXELS);
        assert_eq!(image.texture_descriptor.size.height, TEXELS);
        // A bound texture is looked at from every distance a baked one is, so
        // it carries the same chain; without it the wall sparkles at forty
        // metres.
        assert_eq!(
            image.texture_descriptor.mip_level_count as usize,
            levels(TEXELS),
            "image {} has no mip chain",
            binding.image
        );
    }
    // Nothing past the graph's own images is filled, so Bevy binds its fallback
    // there and the shader never reads it.
    for slot in slots.iter().skip(bindings.len()) {
        assert!(slot.is_none());
    }
}

#[test]
fn a_packed_image_physically_holds_every_lane_its_bindings_read() {
    // An image has one format, so the moment several planes share one they have
    // to share the format too. Left to themselves a `height` plane asks for
    // sixteen-bit single-channel codes and a `roughness` plane for eight unorm
    // bits: an image written in the first and read in the second has three
    // channels that were never written, and the wall renders at roughness zero
    // — a mirror — with no error anywhere. So a packed image is half floats,
    // and this is the round trip that says the bytes are really there.
    let mut harness = Harness::new();
    let material = harness.create(&definition("study:packed", &[]));
    let entry = harness
        .registry
        .get(material.extension.graph)
        .expect("registered");
    let bindings = entry.bindings.clone();
    let textures = entry.report.clone();
    let slots = material.extension.textures();
    let packed = bindings
        .iter()
        .position(|binding| binding.buffers.len() > 1)
        .unwrap_or_else(|| panic!("ten scalar bindings should have packed: {textures}"));
    let binding = &bindings[packed];
    assert_eq!(binding.format, PlaneFormat::Rgba16Float);
    let image = harness.image(slots[packed].expect("a packed image fills its slot"));
    assert_eq!(
        image.texture_descriptor.format,
        texture_format(PlaneFormat::Rgba16Float)
    );
    // Four channels of two bytes, which is what the shader's swizzle reads.
    // Under the format the first plane would have chosen alone there are two
    // bytes in all and three of the four lanes do not exist.
    let texels = (TEXELS as usize) * (TEXELS as usize);
    let level = image.data.as_ref().expect("an uploaded image")[..texels * 8].to_vec();
    assert_eq!(level.len(), texels * 8);
    for lane in 0..binding.buffers.len() {
        let codes: std::collections::BTreeSet<u16> = level
            .as_chunks::<8>()
            .0
            .iter()
            .map(|texel| u16::from_le_bytes([texel[lane * 2], texel[lane * 2 + 1]]))
            .collect();
        assert!(
            codes.len() > 1,
            "lane {lane} of image {} holds one value, so nothing was written into it",
            binding.image
        );
    }
}

#[test]
fn two_definitions_with_the_same_graph_and_live_set_share_a_graph_key() {
    let mut harness = Harness::new();
    // Two different values of the *live* parameter. Live values are a uniform,
    // not a pipeline: the two must be one key, one shader and one set of
    // textures.
    let first = harness.create(&definition(
        "study:strip",
        &[("glow", ashlar::ParamValue::Float(0.5))],
    ));
    let second = harness.create(&definition(
        "study:strip",
        &[("glow", ashlar::ParamValue::Float(3.5))],
    ));
    assert_eq!(first.extension.graph, second.extension.graph);
    assert_eq!(harness.registry.len(), 1);
    assert_eq!(harness.cache.statics(), 1);
    assert_eq!(
        first.extension.static_0.as_ref(),
        second.extension.static_0.as_ref()
    );
    // And the uniform block does differ, because that is where the value went.
    assert_ne!(first.extension.params, second.extension.params);
}

#[test]
fn a_definition_that_folds_a_parameter_differently_is_a_different_graph_key() {
    let mut harness = Harness::new();
    // `tint` is not live: it is folded into the bound textures and into the
    // generated text, so two values of it are two compiled graphs. A digest
    // that ignored this would draw one wall with the other's colour.
    let first = harness.create(&definition(
        "study:strip",
        &[("tint", ashlar::ParamValue::Color([0.62, 0.82, 0.84]))],
    ));
    let second = harness.create(&definition(
        "study:strip",
        &[("tint", ashlar::ParamValue::Color([0.9, 0.2, 0.1]))],
    ));
    assert_ne!(first.extension.graph, second.extension.graph);
    assert_eq!(harness.registry.len(), 2);
    assert_eq!(harness.cache.statics(), 2);
    // Two compiled graphs, and the difference is exactly where the folded
    // value reaches: `tint` multiplies the base colour and nothing else, so
    // the base colour is one image per value and the grain the roughness, the
    // height and the live emissive all read is one image between them. A
    // bound image is keyed by the plane behind it rather than by the graph
    // that asked for it, which is what makes the second half true.
    assert_eq!(differing(&first, &second), 1);
}

/// How many of two materials' bound images are not the same image.
fn differing(left: &ProceduralMaterial, right: &ProceduralMaterial) -> usize {
    left.extension
        .textures()
        .iter()
        .zip(right.extension.textures())
        .filter(|(left, right)| left.map(Handle::id) != right.map(Handle::id))
        .count()
}

#[test]
fn two_graphs_over_one_wall_bake_that_wall_once() {
    // The item this test is for. `study:still-wet` and `study:still-lit` are
    // two compiled graphs — two keys, two shaders, two fragment halves — and
    // one wall: both instance `study:still` and both bind the same planes of
    // it. Keyed by the shader, that wall is rasterised twice; keyed by the
    // plane behind each bound image, once.
    let mut harness = Harness::new();
    let wet = harness.create(&definition("study:still-wet", &[]));
    let rasterised = harness.cache.rasterised();
    assert!(rasterised > 0, "the first graph rasterised nothing");
    let images = harness.cache.bound_images();
    assert!(images > 0, "the first graph bound nothing");

    let lit = harness.create(&definition("study:still-lit", &[]));
    assert_ne!(wet.extension.graph, lit.extension.graph, "two graphs");
    assert_eq!(harness.registry.len(), 2, "two compiled shaders");
    assert_eq!(harness.cache.statics(), 2, "two entries in the cache");
    // And one bake between them, which is the claim.
    assert_eq!(
        harness.cache.rasterised(),
        rasterised,
        "the second graph rasterised the wall again"
    );
    assert_eq!(
        harness.cache.bound_images(),
        images,
        "the second graph made images of its own"
    );
    assert_eq!(
        wet.extension.textures(),
        lit.extension.textures(),
        "two graphs over one wall bound two sets of handles"
    );
}

#[test]
fn two_graphs_that_fold_the_wall_differently_bake_it_twice() {
    // The other half of the rule, and what keeps the sharing honest: the plane
    // key is the wall's own expression *after folding*, so a `Subgraph` that
    // binds `wear` to something else is a different expression and must be a
    // different image. Sharing here would be one building wearing another's
    // texels, which is exactly what the exact-key policy exists to refuse.
    let mut harness = Harness::new();
    let wet = harness.create(&definition("study:still-wet", &[]));
    let rasterised = harness.cache.rasterised();
    let worn = harness.create(&definition("study:still-worn", &[]));

    let again = harness.cache.rasterised() - rasterised;
    assert!(
        again > 0,
        "a differently folded wall was taken from the cache"
    );
    // And only the planes the fold reaches: what `wear` does not touch is
    // still one plane between the two, so the second graph rasterises strictly
    // less than the first did rather than the whole wall again.
    assert!(again < rasterised, "{again} of {rasterised} planes again");
    // `wear` reaches the base colour and nothing else, so the base colour is
    // the one image that differs and the planes it does not reach are still
    // shared. Both halves matter: the first is the correctness, the second is
    // that the key is the *plane*'s and not the graph's.
    let (left, right) = (wet.extension.textures(), worn.extension.textures());
    let differing = left
        .iter()
        .zip(right)
        .filter(|(left, right)| left.map(Handle::id) != right.map(Handle::id))
        .count();
    let shared = left.iter().flatten().count() - differing;
    assert_eq!(differing, 1, "{left:?} against {right:?}");
    assert!(shared > 0, "nothing was shared: {left:?}");
}

#[test]
fn a_shader_surface_naming_an_unknown_graph_fails_read_by_path() {
    let root = TempDir::new().expect("temp dir");
    let error = read_error(root.path(), "Shader(graph: \"study:nothing\", params: {})");
    assert!(error.contains("preflighting"), "{error}");
    assert!(error.contains("material paint"), "{error}");
    assert!(error.contains("study:nothing"), "{error}");
    // And it says what the library does hold, because a typo is the common
    // case and a list is the answer to it.
    assert!(error.contains("study:strip"), "{error}");
}

#[test]
fn a_shader_surface_binding_a_parameter_the_graph_does_not_declare_fails_read_by_path() {
    let root = TempDir::new().expect("temp dir");
    let error = read_error(
        root.path(),
        "Shader(graph: \"study:strip\", params: {\"sparkle\": Float(1.0)})",
    );
    assert!(error.contains("params[sparkle]"), "{error}");
    assert!(error.contains("study:strip"), "{error}");
}

#[test]
fn a_shader_surface_that_compiles_passes_preflight() {
    let root = TempDir::new().expect("temp dir");
    let path = root.path().join("materials.ron");
    std::fs::write(
        &path,
        "(materials: {\"paint\": (surface: Shader(graph: \"study:strip\", \
         params: {\"glow\": Float(2.0)}))})",
    )
    .expect("fixture library");
    let library = ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &graphs())
        .expect("a compiled surface preflights");
    assert!(matches!(
        library.materials["paint"].surface,
        Surface::Shader { .. }
    ));
}

#[test]
fn both_stages_are_registered_under_the_ids_specialize_will_look_up() {
    let mut harness = Harness::new();
    let material = harness.create(&definition("study:strip", &[]));
    let key = material.extension.graph;
    for stage in [Stage::Fragment, Stage::Prepass] {
        let shader = harness
            .shader(shader_id(key, stage))
            .unwrap_or_else(|| panic!("no shader registered for {stage:?}"));
        let source = format!("{:?}", shader.source);
        // The Bevy module, not the core: these go through `naga_oil`, which is
        // how every Bevy shader is loaded, and the imports are what let the
        // fragment call `pbr_input_from_standard_material`.
        assert!(source.contains("#import bevy_pbr::"), "{stage:?}");
        assert!(source.contains("MATERIAL_BIND_GROUP"), "{stage:?}");
        assert!(source.contains("fn fragment("), "{stage:?}");
    }
    // The two stages are two assets, or the prepass would draw the main pass.
    assert_ne!(
        shader_id(key, Stage::Fragment),
        shader_id(key, Stage::Prepass)
    );
}

#[test]
fn the_uniform_block_carries_each_live_value_where_the_layout_puts_it() {
    let mut harness = Harness::new();
    let material = harness.create(&definition(
        "study:strip",
        &[("glow", ashlar::ParamValue::Float(2.75))],
    ));
    let entry = harness
        .registry
        .get(material.extension.graph)
        .expect("registered");
    let slot = entry.layout.entry("glow").expect("a live parameter");
    assert_eq!(slot.offset, 0);
    let bytes = entry.layout.bytes(&[[2.75, 0.0, 0.0]]);
    assert_eq!(GraphParams::from_bytes(&bytes), material.extension.params);
    assert_eq!(material.extension.params.rows[0].x, 2.75);
    // Everything past the graph's own parameters is zero, so the block has the
    // same shape whatever the graph is.
    for row in &material.extension.params.rows[1..] {
        assert_eq!(*row, Vec4::ZERO);
    }
}

#[test]
fn the_block_is_the_fixed_size_the_bind_group_layout_declares() {
    // The block the type declares is the same for every graph, so the layout
    // never changes shape; the generated struct reads the front of it and the
    // rest is padding.
    assert_eq!(PARAM_BYTES, PARAM_ROWS * 16);
    assert_eq!(u64::from(GraphParams::SHADER_SIZE), PARAM_BYTES as u64);
    // And a graph's own block fits inside it with room to spare.
    let material = strip().build().expect("the fixture graph builds");
    let split = partition(&material, &Target::shader_for(&material), TEXELS).expect("a partition");
    let shader = ashlar_material::wgsl::emit(&split, Stage::Fragment).expect("WGSL");
    assert!(shader.layout().size() as usize <= PARAM_BYTES);
}

#[test]
fn the_tiling_of_a_compiled_surface_is_the_tiling_of_a_baked_one() {
    let mut harness = Harness::new();
    let material = harness.create(&definition("study:strip", &[]));
    // `tile_metres` and `uv_offset` go through `uv_transform`, which is the
    // coordinate the generated fragment reads, so moving a surface between a
    // bake and a shader changes nothing about how the wall tiles.
    let expected = bevy::math::Affine2::from_scale_angle_translation(
        Vec2::new(2.4, 1.2).recip(),
        0.0,
        Vec2::new(0.25, 0.0),
    );
    assert_eq!(material.base.uv_transform, expected);
    assert_eq!(material.base.emissive, LinearRgba::rgb(2.0, 3.5, 4.0));
}

#[test]
fn a_graph_with_nothing_live_compiles_to_texture_reads_and_says_to_bake_it() {
    let mut harness = Harness::new();
    let material = harness.create(&definition("study:still", &[]));
    let entry = harness
        .registry
        .get(material.extension.graph)
        .expect("registered");
    assert_eq!(entry.report.ops, 0, "{}", entry.report);
    assert_eq!(entry.report.verdict, "no live inputs: bake this");
    assert!(entry.key.live().is_empty());
    // The uniform block is still there, and still empty: the layout does not
    // change shape from one graph to the next.
    assert_eq!(material.extension.params, GraphParams::default());
}

#[test]
fn create_baked_sends_a_shader_surface_to_create_shader() {
    let mut harness = Harness::new();
    let definition = definition("study:strip", &[]);
    let graphs = graphs();
    let error = harness
        .app
        .world_mut()
        .resource_scope(|world, mut images: Mut<Assets<Image>>| {
            format!(
                "{:#}",
                ashlar_bevy::runtime_bake::create_graph_material(
                    &definition,
                    world.resource::<AssetServer>(),
                    &mut ashlar_bevy::runtime_bake::BakeContext {
                        graphs: &graphs,
                        cache: &mut BakeCache::new(),
                        images: &mut images,
                        threads: THREADS,
                        baker: Baker::Cpu,
                    },
                )
                .expect_err("a compiled graph is not a StandardMaterial")
            )
        });
    assert!(error.contains("create_shader_material"), "{error}");
    assert!(error.contains("study:strip"), "{error}");
}

#[test]
fn create_shader_refuses_a_surface_that_is_not_a_shader() {
    let mut harness = Harness::new();
    let error = format!(
        "{:#}",
        harness
            .try_create(&MaterialDefinition::default())
            .expect_err("a Plain surface compiles to nothing")
    );
    assert!(error.contains("Surface::Shader"), "{error}");
}

#[test]
fn a_key_describes_itself_well_enough_to_tell_two_graphs_apart() {
    let material = strip().build().expect("builds");
    let split = partition(&material, &Target::shader_for(&material), TEXELS).expect("a partition");
    let key = ShaderKey::new("study:strip", TEXELS, &material, &split);
    let described = key.describe();
    assert!(described.contains("study:strip"), "{described}");
    assert!(described.contains("glow"), "{described}");
    assert!(described.contains("256"), "{described}");
    // The digest is the key's own, and stable within and across runs.
    assert_eq!(
        key.digest(),
        ShaderKey::new("study:strip", TEXELS, &material, &split).digest()
    );
    assert_ne!(
        key.digest(),
        ShaderKey::new("study:other", TEXELS, &material, &split).digest()
    );
}

#[test]
fn the_default_resolution_is_a_resolution_a_bake_takes() {
    assert!(DEFAULT_RESOLUTION.is_power_of_two());
    assert!(
        (ashlar_material::bake::MIN_RESOLUTION..=ashlar_material::bake::MAX_RESOLUTION)
            .contains(&DEFAULT_RESOLUTION)
    );
}

#[test]
fn an_extension_declares_one_binding_pair_per_texture_the_partition_may_bind() {
    // The count is in the type, so it cannot depend on the graph. Eight is what
    // the partition refuses above, and the two numbers have to agree or a graph
    // that partitions would fail to bind.
    let extension = GraphExtension::default();
    assert_eq!(
        extension.textures().len(),
        ashlar_material::partition::MAX_BOUND_TEXTURES
    );
    assert_eq!(
        ashlar_bevy::shader::MAX_TEXTURES,
        ashlar_material::partition::MAX_BOUND_TEXTURES
    );
}

#[test]
fn a_live_parameter_the_graph_declares_stays_live_without_a_target_saying_so() {
    // The author's declaration is a floor, and this is the adapter's half of
    // that claim: nothing here passes a live set, and `glow` is live anyway.
    let material = strip().build().expect("builds");
    let live: BTreeMap<&str, Exposure> = material
        .graph()
        .params
        .iter()
        .map(|param| (param.name.as_str(), param.exposure))
        .collect();
    assert_eq!(live["glow"], Exposure::Live);
    assert_eq!(live["tint"], Exposure::Bake);
}

/// Print the generated text for reading. Not a case; run it with
/// `cargo test -p ashlar-bevy --test shaders -- --ignored --nocapture print_the_strip`.
#[test]
#[ignore = "a printer, not a case"]
fn print_the_strip() {
    let material = strip().build().expect("builds");
    let split = partition(&material, &Target::shader_for(&material), TEXELS).expect("a partition");
    println!("{}", split.report());
    for stage in [Stage::Fragment, Stage::Prepass] {
        println!("==== {stage:?} ====");
        println!(
            "{}",
            ashlar_material::wgsl::emit(&split, stage)
                .expect("WGSL")
                .module()
        );
    }
}

#[test]
fn an_instance_override_of_a_live_parameter_is_one_pipeline_and_two_uniforms() {
    // The cheap half of a per-instance override, and the reason a compiled
    // surface takes one at all: a live parameter is a uniform, so two
    // instances over one definition at two values share the shader, the
    // pipeline key and every bound texture, and differ in a block of bytes
    // nothing else ever saw.
    let mut harness = Harness::new();
    let definitions = {
        let mut library = ashlar::MaterialLibrary::default();
        library
            .materials
            .insert("lit".into(), definition("study:strip", &[]));
        library
    };
    let dressed = |glow: Option<f32>| {
        let mut binding = ashlar::Binding::new("lit");
        if let Some(glow) = glow {
            binding = binding.param("glow", ashlar::ParamValue::Float(glow));
        }
        ashlar_bevy::definition(&definitions, &binding)
            .expect("a bound definition")
            .into_owned()
    };
    let graph = harness.create(&dressed(None));
    let dim = harness.create(&dressed(Some(0.2)));
    let bright = harness.create(&dressed(Some(3.5)));

    assert_eq!(dim.extension.graph, bright.extension.graph);
    assert_eq!(dim.extension.graph, graph.extension.graph);
    assert_eq!(harness.registry.len(), 1, "one graph, one compiled shader");
    assert_eq!(
        harness.cache.statics(),
        1,
        "one set of bound textures between them"
    );
    for (left, right) in dim
        .extension
        .textures()
        .iter()
        .zip(bright.extension.textures())
    {
        assert_eq!(left, &right, "an override of a live value rebakes nothing");
    }
    // The uniform block is the whole of the difference, and it carries the
    // value the binding named rather than the one the definition did.
    assert_ne!(dim.extension.params, bright.extension.params);
    let entry = harness
        .registry
        .get(dim.extension.graph)
        .expect("a registered graph");
    let at = |material: &ProceduralMaterial| {
        let entry = entry.layout.entries().iter().find(|e| e.name == "glow");
        let offset = entry.expect("glow is live").offset as usize;
        material.extension.params.rows[offset / 16][(offset % 16) / 4]
    };
    assert_eq!(at(&dim), 0.2);
    assert_eq!(at(&bright), 3.5);
    // The graph's own default is what a binding that overrides nothing carries.
    assert_eq!(at(&graph), 0.6);
}

#[test]
fn an_instance_override_of_a_folded_parameter_is_a_second_compiled_graph() {
    // The other half, and the cost the design names: a parameter that is not
    // live went into the bound textures and into the generated text, so an
    // instance that wants another value wants another compiled graph. Refusing
    // it would be worse — a folded override is exactly what a per-building seed
    // on a compiled surface is — so it is allowed and it is counted.
    let mut harness = Harness::new();
    let definitions = {
        let mut library = ashlar::MaterialLibrary::default();
        library
            .materials
            .insert("strip".into(), definition("study:strip", &[]));
        library
    };
    let dressed = |tint: Option<[f32; 3]>| {
        let mut binding = ashlar::Binding::new("strip");
        if let Some(tint) = tint {
            binding = binding.param("tint", ashlar::ParamValue::Color(tint));
        }
        ashlar_bevy::definition(&definitions, &binding)
            .expect("a bound definition")
            .into_owned()
    };
    let first = harness.create(&dressed(None));
    let second = harness.create(&dressed(Some([0.9, 0.3, 0.2])));
    let same = harness.create(&dressed(Some([0.9, 0.3, 0.2])));

    assert_ne!(first.extension.graph, second.extension.graph);
    assert_eq!(second.extension.graph, same.extension.graph);
    assert_eq!(harness.registry.len(), 2, "two folded values, two shaders");
    assert_eq!(harness.cache.statics(), 2, "and two sets of bound textures");
    assert_eq!(
        second.extension.textures(),
        same.extension.textures(),
        "two instances at one folded value share everything"
    );
    assert_eq!(
        differing(&first, &second),
        1,
        "the tint is folded into the bound base colour and reaches nothing else"
    );
}

// ----------------------------------- the chain a compiled material binds

/// Texels a side for the two cases that read a chain rather than a format.
///
/// [`DEFAULT_RESOLUTION`], which is what the study actually ships at, and it
/// matters here where it does not elsewhere: how much coherence a footprint
/// loses is a fact about texel size. The same wall at 256 loses less than one
/// eight-bit code anywhere, which is the right answer for a wall that size and
/// no test of anything.
const SHIPPED_TEXELS: u32 = DEFAULT_RESOLUTION;

/// The default library, which is where the graphs with real relief live.
fn study() -> MaterialGraphLibrary {
    ashlar_material::stdlib::graphs()
}

/// The 2x2 box filter of a square plane of vectors.
///
/// [`ashlar_material::mips`]'s own rule, written out here rather than reached
/// for, so that what the coherence is compared against is an independent answer
/// and not the same code run twice.
fn halve(plane: &[[f32; 3]], resolution: usize) -> Vec<[f32; 3]> {
    let half = (resolution / 2).max(1);
    let mut out = Vec::with_capacity(half * half);
    for y in 0..half {
        for x in 0..half {
            let (top, bottom) = (2 * y * resolution, (2 * y + 1) * resolution);
            let mut sum = [0.0_f32; 3];
            for index in [
                top + 2 * x,
                top + 2 * x + 1,
                bottom + 2 * x,
                bottom + 2 * x + 1,
            ] {
                for (lane, value) in plane[index].iter().enumerate() {
                    sum[lane] += value * 0.25;
                }
            }
            out.push(sum);
        }
    }
    out
}

/// A vector's length, clamped where a mean of unit vectors cannot go.
fn coherence(vector: [f32; 3]) -> f32 {
    vector
        .iter()
        .map(|lane| lane * lane)
        .sum::<f32>()
        .sqrt()
        .clamp(0.0, 1.0)
}

/// Where one level of a chain starts, in texels, and how many texels it holds.
fn level_span(resolution: u32, level: usize) -> (usize, usize) {
    let mut start = 0;
    let mut size = resolution as usize;
    for _ in 0..level {
        start += size * size;
        size = (size / 2).max(1);
    }
    (start, size * size)
}

/// One graph of the study, lowered and split at `texels`.
fn split_of(graphs: &MaterialGraphLibrary, key: &str, texels: u32) -> Partition {
    let material = graphs.build(key).expect("the study's graphs build");
    partition(&material, &Target::shader_for(&material), texels).expect("and partition")
}

/// The study's own bake of one graph, at `texels`, with the chain.
fn baked<'a>(
    graphs: &'a MaterialGraphLibrary,
    key: &str,
    texels: u32,
    mips: bool,
) -> BakeRequest<'a> {
    BakeRequest {
        graph: graphs.get(key).expect("the study ships it"),
        library: graphs,
        params: &EMPTY_PARAMS,
        resolution: texels,
        mips,
        threads: THREADS,
    }
}

/// The parameter map a bake of a study graph binds: none, so every parameter
/// sits at the graph's own default — which is where a compiled material's
/// uniform starts too, and so what makes the two comparable.
static EMPTY_PARAMS: std::sync::LazyLock<BTreeMap<String, ashlar_material::ParamValue>> =
    std::sync::LazyLock::new(BTreeMap::new);

#[test]
fn a_bound_normal_maps_alpha_is_the_coherence_the_bake_widens_by() {
    // The whole of what a compiled material needs to widen its roughness with
    // distance, and the one number a bake never had to write down anywhere a
    // shader could reach: `|n_avg|`, the length of the mean of the level-0
    // normals over a level's footprint.
    //
    // A bake widens as it filters, because it holds the normal plane and the
    // roughness plane at once. A partition binds separate images that know
    // nothing of each other, so the coherence rides in the alpha channel of the
    // normal map's own chain — the channel a tangent-space normal does not use
    // — and the generated fragment reads it back at whatever level the hardware
    // chose. This is the claim that what is in that channel is the number the
    // bake would have used, level for level.
    let graphs = study();
    let mut harness = Harness::over(study());
    let material = harness.create_at(&definition("library:formed-concrete", &[]), SHIPPED_TEXELS);
    let split = split_of(&graphs, "library:formed-concrete", SHIPPED_TEXELS);
    assert!(
        split.report().widens,
        "a wall with a bound normal map widens: {}",
        split.report()
    );
    let normal = harness.ports(&split, &material);
    let bytes = harness
        .image(normal.get("normal").expect("concrete binds a normal"))
        .data
        .as_deref()
        .expect("an uploaded image")
        .to_vec();

    // The reference: the bake's own level-0 normals, box filtered by hand.
    let (planes, _) = ashlar_material::bake::rasterise(&baked(
        &graphs,
        "library:formed-concrete",
        SHIPPED_TEXELS,
        false,
    ))
    .expect("the concrete bakes");
    let mut mean = planes.normal.clone();
    let mut size = SHIPPED_TEXELS as usize;
    let mut widest = 0.0_f32;
    let mut lost = 0;
    for level in 0..levels(SHIPPED_TEXELS) {
        let (start, texels) = level_span(SHIPPED_TEXELS, level);
        assert_eq!(texels, size * size, "level {level} is the wrong size");
        for (index, texel) in bytes[start * 4..(start + texels) * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            widest = widest.max((coherence(mean[index]) - f32::from(texel[3]) / 255.0).abs());
            if level == 0 {
                assert_eq!(texel[3], 255, "level 0 is unit normals, so coherence one");
            } else if texel[3] < 255 {
                lost += 1;
            }
        }
        if size > 1 {
            mean = halve(&mean, size);
            size /= 2;
        }
    }
    // One eight-bit step, which is all the channel holds.
    assert!(
        widest <= 1.0 / 255.0,
        "a bound normal map's alpha is not the bake's own coherence: worst {widest}"
    );
    assert!(
        lost > 0,
        "nothing in a cast concrete wall's chain lost a code of coherence, so there is \
         nothing for the roughness to widen by"
    );
}

#[test]
fn a_bound_roughness_is_the_un_widened_one_the_fragment_widens() {
    // The other half of the pairing, and the reason the widening is emitted
    // into the fragment rather than baked into this image. A bake's ORM holds
    // the *widened* roughness at every level below the first; a bound roughness
    // map holds the plain box filter, and the fragment adds the term.
    //
    // `library:paving-slabs` because its finest lattice fits the 256 texels
    // this test binds at.
    let graphs = study();
    let mut harness = Harness::over(study());
    let material = harness.create_at(&definition("library:paving-slabs", &[]), TEXELS);
    let split = split_of(&graphs, "library:paving-slabs", TEXELS);
    assert!(split.report().widens, "{}", split.report());
    let ports = harness.ports(&split, &material);
    let bound = harness
        .image(ports.get("roughness").expect("paving binds a roughness"))
        .data
        .as_deref()
        .expect("an uploaded image")
        .to_vec();
    let orm = ashlar_material::bake::bake(&baked(&graphs, "library:paving-slabs", TEXELS, true))
        .expect("the paving bakes")
        .orm;

    // Level 0: the bound map's red channel is the ORM's green channel, code for
    // code. Nothing about the widening moved it, and nothing may.
    let texels = (TEXELS as usize) * (TEXELS as usize);
    let differing = bound[..texels * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(orm.mips[0].as_chunks::<4>().0)
        .filter(|(bound, orm)| bound[0] != orm[1])
        .count();
    assert_eq!(
        differing, 0,
        "the bound roughness at level 0 is not the roughness the bake wrote"
    );

    // Level 3: the bake is rougher wherever it differs at all, because it has
    // folded in a coherence the bound map leaves to the shader.
    let (start, texels) = level_span(TEXELS, 3);
    let mut rougher = 0;
    for (bound, orm) in bound[start * 4..(start + texels) * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(orm.mips[3].as_chunks::<4>().0)
    {
        assert!(
            orm[1] >= bound[0],
            "the bake's level 3 roughness is below the box filter it widened"
        );
        rougher += usize::from(orm[1] > bound[0]);
    }
    assert!(
        rougher > 0,
        "the bake widened nothing at level 3, so there is nothing to have matched"
    );
}
