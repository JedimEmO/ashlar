//! The material sheet's claim, without a window: a pair is one material.
//!
//! Each column of a sheet is two specimens of the same material — the graph
//! baked into images, and the same graph partitioned and compiled.
//! What makes that a *pair* rather than two materials that happen to stand
//! beside each other is that the compiled half's bound textures are the maps
//! the bake wrote, encoded and mipped the same way. This is where that is
//! checked as bytes; the sheet capture is where it is checked by eye, and
//! `just conformance` is where the arithmetic behind it is checked per op on a
//! GPU.
//!
//! One thing this does not claim, stated in [`ashlar_showcase::sheet`]'s own
//! documentation and visible in the far column of a capture: the ORM map is
//! packed by the bake and left as three images by the partition, so its
//! quantisation differs.
//!
//! And one place a bound map is deliberately *not* the bake's bytes. A normal
//! map's alpha is 255 everywhere in a bake and carries `|n_avg|` in a bound
//! image, because that is where a compiled material keeps the Toksvig term the
//! bake folds into its own roughness chain. Level 0 agrees — unit normals are
//! coherence one — and every level below it differs on purpose, in the one
//! channel a tangent-space normal does not use. So the comparison below is the
//! three channels that are the normal, and then the fourth in its own right.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "shipped content and generated fixtures, and a uniform lane that has to \
              carry exactly the number the definition wrote"
)]
use std::{collections::BTreeMap, num::NonZeroUsize};

use ashlar::{Bake, MaterialDefinition, Surface};
use ashlar_bevy::{
    runtime_bake::{BakeCache, BakeContext, Baker, create_graph_material},
    shader::{GraphShaders, ProceduralMaterial, ShaderContext, create_shader_material},
};
use ashlar_material::{MaterialGraphLibrary, ir::Target, partition::partition};
use ashlar_showcase as showcase;
use bevy::{MinimalPlugins, asset::AssetPlugin, prelude::*, shader::Shader};

/// Texels a side. A sixteenth of the work of the shipped 1024 and the same
/// claim: what is compared is the encoder and the chain, not the texel count.
const TEXELS: u32 = 256;

/// Rows a bake divides across. Explicit and small, for the reason every other
/// bake in this workspace says.
const THREADS: Option<NonZeroUsize> = NonZeroUsize::new(8);

/// The materials this compares: the two library surfaces whose finest lattice
/// fits this test's [`TEXELS`], both with buffered filters in them — a slab
/// lattice's jump flood and an occlusion march — so those are on the compiled
/// side too.
const COMPARED: [&str; 2] = ["formed-concrete", "paving-slabs"];

fn library() -> MaterialGraphLibrary {
    showcase::materials::graphs()
}

/// Everything both entry points need, in one place.
struct Harness {
    app: App,
    graphs: MaterialGraphLibrary,
    cache: BakeCache,
    registry: GraphShaders,
}

impl Harness {
    fn new() -> Self {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<Shader>();
        Self {
            app,
            graphs: library(),
            cache: BakeCache::new(),
            registry: GraphShaders::new(),
        }
    }

    /// The baked half, at this test's resolution.
    fn bake(&mut self, definition: &MaterialDefinition) -> StandardMaterial {
        let Surface::Graph(bake) = &definition.surface else {
            panic!("the baked half of a pair is baked");
        };
        let baked = MaterialDefinition {
            surface: Surface::Graph(Bake {
                resolution: TEXELS,
                ..bake.clone()
            }),
            ..definition.clone()
        };
        let (graphs, cache) = (&self.graphs, &mut self.cache);
        self.app
            .world_mut()
            .resource_scope(|world, mut images: Mut<Assets<Image>>| {
                let server = world.resource::<AssetServer>().clone();
                create_graph_material(
                    &baked,
                    &server,
                    &mut BakeContext {
                        graphs,
                        cache,
                        images: &mut images,
                        threads: THREADS,
                        baker: Baker::Cpu,
                    },
                )
            })
            .expect("a shipped graph bakes")
    }

    /// The compiled half.
    fn compile(&mut self, definition: &MaterialDefinition) -> ProceduralMaterial {
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
            .expect("a shipped graph compiles")
    }

    /// Every bound texture of a compiled material, by the PBR port it is.
    ///
    /// The material carries handles in binding order and nothing else; which
    /// port each one *is* is the partition's answer, so the partition is run
    /// again here over the same graph at the same resolution. It is a lowering
    /// and costs nothing next to the two bakes either side of it.
    fn ports(
        &self,
        definition: &MaterialDefinition,
        material: &ProceduralMaterial,
    ) -> BTreeMap<String, Handle<Image>> {
        let Surface::Shader { graph, params } = &definition.surface else {
            panic!("the compiled half of a pair is a Shader surface");
        };
        let built = self
            .graphs
            .get(graph)
            .unwrap()
            .with_params(params)
            .unwrap()
            .build_in(&self.graphs)
            .unwrap();
        let split = partition(&built, &Target::shader_for(&built), TEXELS).unwrap();
        let entry = self.registry.get(material.extension.graph).unwrap();
        let handles = material.extension.textures();
        let mut by_port = BTreeMap::new();
        for (index, binding) in entry.bindings.iter().enumerate() {
            let bound = &split.textures()[binding.buffers[0].index()];
            if let (Some(port), Some(handle)) = (bound.port.as_ref(), handles[index]) {
                by_port.insert(port.clone(), handle.clone());
            }
        }
        by_port
    }

    fn bytes(&self, handle: &Handle<Image>) -> &[u8] {
        self.app
            .world()
            .resource::<Assets<Image>>()
            .get(handle)
            .expect("a bound texture is an asset")
            .data
            .as_deref()
            .expect("a baked image carries its own bytes")
    }

    fn report(&self, material: &ProceduralMaterial) -> String {
        self.registry
            .get(material.extension.graph)
            .expect("a compiled material is registered")
            .report
            .to_string()
    }
}

/// The two halves of one column of a sheet.
fn pair(set: &str, distance: &str) -> (MaterialDefinition, MaterialDefinition) {
    let library = showcase::sheet::definitions(set);
    let take = |delivery: &str| {
        library.materials[&showcase::sheet::material_key(set, distance, delivery)].clone()
    };
    (take("baked"), take("live"))
}

#[test]
fn the_compiled_half_of_a_pair_binds_the_maps_the_bake_wrote() {
    let mut harness = Harness::new();
    let mut compared = 0;
    // Texels below level 0 whose footprint's normals disagreed enough to cost a
    // code of coherence, over every set. Counted across the loop rather than
    // asserted inside it because a *flat* map is entitled to lose nothing; the
    // paving has joints, and a chain of a joint had better lose something.
    let mut lost = 0;
    for set in COMPARED {
        let (baked, live) = pair(set, "near");
        let standard = harness.bake(&baked);
        let compiled = harness.compile(&live);
        // Nothing this graph declares is live, so the compiler says what the
        // showcase already does with it.
        assert!(
            harness
                .report(&compiled)
                .starts_with("no live inputs: bake this"),
            "{set}: {}",
            harness.report(&compiled)
        );
        let ports = harness.ports(&live, &compiled);
        // The two maps the two backends write the same way, whole chain
        // included. Base colour goes through the same sRGB transfer, the same
        // ordered dither and the same wrapped box filter; the normal is derived
        // from the same height by the same central difference, written half and
        // half about zero and renormalised at every level below the first.
        //
        // The normal's alpha is the exception and it is compared below: a
        // compiled material has nowhere but that channel to keep the coherence
        // its roughness widens by, and a baked one does not need it there.
        for (port, baked_handle, channels) in [
            ("base_color", standard.base_color_texture.as_ref(), 0..4),
            ("normal", standard.normal_map_texture.as_ref(), 0..3),
        ] {
            let baked_handle = baked_handle.unwrap_or_else(|| panic!("{set} bakes a {port}"));
            let bound = ports
                .get(port)
                .unwrap_or_else(|| panic!("{set} binds no {port}: {:?}", ports.keys()));
            let (mine, theirs) = (harness.bytes(bound), harness.bytes(baked_handle));
            assert_eq!(
                mine.len(),
                theirs.len(),
                "{set}/{port}: the two chains are different sizes"
            );
            // Byte for byte but for rounding: a value computed inline on one
            // side and read back from a bound plane on the other can land a
            // hair either side of a code boundary. The paving's jump flood does
            // that on two texels in eighty-seven thousand, by one code. More
            // texels, or more than a code, is a different picture.
            let mut worst = 0_u8;
            let differing = mine
                .as_chunks::<4>()
                .0
                .iter()
                .zip(theirs.as_chunks::<4>().0)
                .filter(|(mine, theirs)| {
                    channels.clone().any(|lane| {
                        worst = worst.max(mine[lane].abs_diff(theirs[lane]));
                        mine[lane] != theirs[lane]
                    })
                })
                .count();
            assert!(
                differing <= 4 && worst <= 1,
                "{set}/{port}: the compiled half is not the map the bake wrote: \
                 {differing} texels, worst {worst} codes"
            );
            compared += 1;
        }
        // The channel that is not the normal. A KTX2 writes an opaque 255 at
        // every level; a bound image writes `|n_avg|`, the length of the mean
        // of the level-0 normals over that level's footprint, which is one at
        // level 0 and shorter below it. That is the whole of the Toksvig term,
        // and carrying it here is what lets the fragment widen a roughness the
        // bake widened as it filtered.
        let normal = harness.bytes(
            ports
                .get("normal")
                .unwrap_or_else(|| panic!("{set} binds no normal")),
        );
        let texels = (TEXELS as usize) * (TEXELS as usize);
        let (level0, below) = normal.split_at(texels * 4);
        assert!(
            level0
                .as_chunks::<4>()
                .0
                .iter()
                .all(|texel| texel[3] == 255),
            "{set}: level 0 of a normal map is unit vectors, so its coherence is one"
        );
        lost += below
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|texel| texel[3] < 255)
            .count();
        // The ORM is the one map that is deliberately not comparable: the bake
        // packs three fields into one image whatever they are, and the
        // partition binds only the ones that are fields at all — a constant
        // metalness is written into the shader as a literal and not bound.
        // That is a texture fetch saved rather than a difference in the
        // picture.
        assert!(standard.metallic_roughness_texture.is_some(), "{set}");
        assert!(ports.contains_key("roughness"), "{set}: {:?}", ports.keys());
    }
    assert_eq!(compared, COMPARED.len() * 2);
    assert!(
        lost > 0,
        "no bound normal map below level 0 lost any coherence, so the chain carries \
         nothing for a compiled roughness to widen by"
    );
}

#[test]
fn the_three_columns_of_a_sheet_share_one_set_of_bound_textures() {
    // What the exact key buys, and the literal claim the step asked for: the
    // three compiled specimens of a set differ only in `tile_metres`, which is
    // a constant of the material and not of the graph, so all three are one
    // compiled graph, one pipeline and one set of images — handle for handle.
    let mut harness = Harness::new();
    let first = {
        let (_, live) = pair("formed-concrete", showcase::sheet::DISTANCES[0].0);
        let material = harness.compile(&live);
        harness.ports(&live, &material)
    };
    assert!(!first.is_empty());
    for (distance, _) in showcase::sheet::DISTANCES {
        let (_, live) = pair("formed-concrete", distance);
        let material = harness.compile(&live);
        assert_eq!(harness.ports(&live, &material), first, "{distance}");
    }
    assert_eq!(harness.registry.len(), 1, "one graph, one compiled entry");
    assert_eq!(harness.cache.statics(), 1, "one graph, one set of images");
}

#[test]
fn the_wet_concrete_instances_its_dry_wall_and_costs_a_handful_of_ops() {
    // The other half of what a pair is for. `library:formed-concrete` is baked
    // for every wall of the showcase; `showcase:concrete-wet` instances that
    // same graph — four subgraph nodes reading four of its outputs, inlined and
    // shared — and adds what only a compiled material can have: a live
    // `wetness`, gated by the fragment's own world normal and height. It is a
    // second graph rather than a second parameter because a bake refuses a
    // world-space input outright, and the wall it instances is baked.
    let mut harness = Harness::new();
    let library = showcase::materials::graphs();
    let study = showcase::library::materials();
    let dry = &study.materials["library:formed-concrete"];
    let wet = &study.materials["showcase:concrete-wet"];
    let (Surface::Graph(bake)
    | Surface::Files {
        baked_from: Some(bake),
        ..
    }) = &dry.surface
    else {
        panic!("the study's concrete records its bake");
    };
    let Surface::Shader { graph, params } = &wet.surface else {
        panic!("the study's wet concrete is compiled");
    };
    assert_eq!(graph, "showcase:concrete-wet");
    // And the graph it names is the dry wall's, instanced: one surface, two
    // deliveries, with the world-space gate the only thing between them.
    let wet_graph = library
        .get(graph)
        .expect("the wet concrete is in the library");
    let instanced: Vec<&str> = wet_graph
        .nodes
        .values()
        .filter_map(|node| match node {
            ashlar_material::Node::Subgraph(subgraph) => Some(subgraph.graph.as_str()),
            _ => None,
        })
        .collect();
    assert!(!instanced.is_empty(), "the wet concrete instances nothing");
    assert!(
        instanced.iter().all(|key| *key == bake.graph),
        "{instanced:?} is not the wall the study bakes"
    );
    let ashlar::ParamValue::Float(wetness) = params["wetness"] else {
        panic!("wetness is a float");
    };
    assert!((0.0..=1.0).contains(&wetness) && wetness > 0.5, "{wetness}");
    let dry_graph = library.get(&bake.graph).expect("the dry wall's graph");
    for (name, value) in &bake.params {
        let param = dry_graph.params.iter().find(|p| &p.name == name);
        assert_eq!(
            param.map(|p| &p.value),
            Some(value),
            "the dry wall is the graph's default: {name}"
        );
    }

    let compiled = harness.compile(wet);
    let report = harness.report(&compiled);
    assert!(report.starts_with("live: 1 uniform"), "{report}");
    // Two mixes over two bound maps, two world masks and the multiply that
    // gates one by the other: a live parameter at the very end of a graph costs
    // what the design said it would, and the gate is a lane and a ramp each.
    let entry = harness.registry.get(compiled.extension.graph).unwrap();
    assert_eq!(entry.report.params, 1);
    assert!(entry.report.ops <= 32, "{report}");
    assert!(entry.report.ops_per_output["base_color"] > 0, "{report}");
    assert!(entry.report.ops_per_output["roughness"] > 0, "{report}");
    for still in ["occlusion", "height", "normal"] {
        assert_eq!(entry.report.ops_per_output.get(still), Some(&0), "{report}");
    }
    // And the uniform really carries the definition's value rather than the
    // graph's default, which is what makes the threshold wet and the wall dry.
    let at = entry.layout.entry("wetness").expect("a live parameter");
    assert_eq!(at.offset, 0);
    assert_eq!(compiled.extension.params.rows[0].x, wetness);
}
