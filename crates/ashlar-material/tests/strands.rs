//! What a strand layer scatters, and the four promises the set makes.
//!
//! The promises are the ones a level-of-detail chain rests on, so each one is a
//! test rather than a paragraph: the set does not depend on how the rows were
//! divided, it meets itself at the seam, the density is a threshold rather than
//! a fade, and a rank prefix is a *subset* of a looser one. The rest of the file
//! is refusals — a field that reaches the frame, a layer nobody declared, a
//! constant outside its range — each checked by the path it names, because a
//! surface that will not scatter should say which field of which layer is wrong.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
#![allow(
    clippy::cast_precision_loss,
    reason = "strand counts in these fixtures are two-digit"
)]
#![allow(
    clippy::float_cmp,
    reason = "a layer that asks for no facing, no sink and no clump gets exactly \
              none of them; a margin would pass one that had quietly started \
              carrying a little of each"
)]
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use ashlar_material::{
    Input, MaterialGraph, MaterialGraphLibrary, ParamValue, PbrOutput, Period, StrandLayer,
    StrandProfile, SurfaceOutput,
    interp::{Inputs, Interpreter},
    ir::{Target, lower_strands},
    nodes::{Math, MathOp, Noise, Subgraph, Time, Transform, Uv, Voronoi, VoronoiOutput, WorldPos},
    strands::{StrandError, StrandRequest, StrandSet, scatter},
};

/// Six decimals, which is the last place an `f32` expression this short holds.
fn close(left: f32, right: f32) -> bool {
    left.to_bits() == right.to_bits() || (left - right).abs() < 1e-6
}

/// The planes a buffered field would want, at the smallest size a bake allows:
/// nothing here rasterises one unless the case says so.
const FIELDS: u32 = 256;

/// Two threads, named rather than defaulted, because the machine running this
/// is busy and its thirty-two cores are not the point of any case here.
fn threads() -> Option<NonZeroUsize> {
    NonZeroUsize::new(2)
}

fn no_params() -> BTreeMap<String, ParamValue> {
    BTreeMap::new()
}

/// What fraction of a whole a count is, as a `f64` a range test can read.
///
/// Through `u32` rather than casting the `usize` straight across, because a
/// lattice is bounded by `MAX_PERIOD` squared and `f64::from` is then exact
/// where a cast is a lint about a count this can never hold.
fn share(part: usize, whole: usize) -> f64 {
    let count = |value: usize| f64::from(u32::try_from(value).unwrap());
    count(part) / count(whole).max(1.0)
}

/// Scatter one layer of a graph, with the thread count named.
fn sown(graph: &MaterialGraph, layer: &str, count: Option<NonZeroUsize>) -> StrandSet {
    scattered(graph, layer, count).unwrap()
}

fn scattered(
    graph: &MaterialGraph,
    layer: &str,
    count: Option<NonZeroUsize>,
) -> Result<StrandSet, StrandError> {
    let library = MaterialGraphLibrary::default();
    let params = no_params();
    scatter(&StrandRequest {
        graph,
        library: &library,
        params: &params,
        layer,
        field_resolution: FIELDS,
        threads: count,
    })
}

/// A lawn: a tiling noise under the length, the colour and the density, with
/// every per-strand variation left off so that a field's value reaches a strand
/// unchanged and a test can compare the two.
fn lawn(count: u32) -> MaterialGraph {
    MaterialGraph::builder("test:lawn")
        .node("field", Noise::value().period(4))
        .output(PbrOutput::new().roughness("field"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(count)
                .length("field")
                .colors("field", [1.0, 1.0, 1.0])
                .metres(1.0, 1.0),
        )
        .into_graph()
}

#[test]
fn a_set_is_the_same_strands_however_the_rows_were_divided() {
    // The claim the whole feature rests on: a chunk of grass built on one
    // machine and the same chunk built on another are the same blades, so a
    // strand set can be rebuilt rather than stored, and a LOD level built at a
    // different moment lines up with the one it replaces.
    let graph = lawn(16);
    let one = sown(&graph, "blades", NonZeroUsize::new(1));
    for count in [1, 2, 3, 7, 64] {
        assert_eq!(
            sown(&graph, "blades", NonZeroUsize::new(count)),
            one,
            "{count} threads"
        );
    }
    assert_eq!(sown(&graph, "blades", None), one, "the default division");
    assert_eq!(sown(&graph, "blades", threads()), one, "and a second run");
}

#[test]
fn a_strand_reads_its_fields_at_its_own_root_and_they_meet_themselves_at_the_seam() {
    // Two claims in one loop, because separating them would need the same
    // fixture twice. The first is that the number on the strand is the field at
    // the root the strand reports, rather than at a cell centre or a texel near
    // it. The second is the tiling rule: the same field a whole UV unit along
    // is the same number, so the strand at the left edge of the repeat and its
    // wrapped twin agree and the seam has no line of short blades down it.
    let graph = lawn(8);
    let set = sown(&graph, "blades", threads());
    let material = graph.build().unwrap();
    let ir = lower_strands(&material, "blades", Target::Bake).unwrap();
    let length = ir.root("length").unwrap();
    let reader = Interpreter::new(&ir);
    let at = |uv: [f32; 2]| reader.eval_float(uv, &Inputs::default(), length).unwrap();

    let mut nearest = f32::INFINITY;
    for strand in set.strands() {
        let here = at(strand.root);
        assert!(
            close(strand.length, here),
            "{:?}: {} against the field's {here}",
            strand.root,
            strand.length
        );
        for twin in [
            [strand.root[0] + 1.0, strand.root[1]],
            [strand.root[0], strand.root[1] + 1.0],
            [strand.root[0] - 1.0, strand.root[1] - 1.0],
        ] {
            assert!(
                close(here, at(twin)),
                "{:?} and its twin {twin:?} disagree: {here} against {}",
                strand.root,
                at(twin)
            );
        }
        nearest = nearest.min(strand.root[0]);
    }
    assert!(
        nearest < 0.125,
        "no strand landed in the first cell, so nothing was read near the seam: {nearest}"
    );
}

#[test]
fn a_root_stays_in_its_own_cell_and_every_root_is_inside_the_repeat() {
    let graph = lawn(8);
    let set = sown(&graph, "blades", threads());
    for strand in set.strands() {
        for axis in strand.root {
            assert!(
                (0.0..1.0).contains(&axis),
                "{:?} left the repeat",
                strand.root
            );
        }
    }
    // Jitter is half a cell either way, so a root never crosses into its
    // neighbour and the nine-cell neighbourhood a placement searches is enough.
    for strand in set.strands() {
        for axis in strand.root {
            let centre = ((axis * 8.0).floor() + 0.5) / 8.0;
            assert!(
                (axis - centre).abs() <= 0.5 / 8.0 + 1e-6,
                "{:?} strayed past its own cell",
                strand.root
            );
        }
    }
}

#[test]
fn a_density_of_a_half_keeps_about_half_the_strands_rather_than_shrinking_them() {
    // A threshold, not a multiply: at half density there are half as many
    // strands and every one of them is the length it would have been. Pebbles
    // that dissolve read as a mistake and grass that shrinks reads as a lawn
    // mown by a drunk, which is the argument `Tile::mask` makes at length.
    let full = |density: f32| {
        MaterialGraph::builder("test:thinned")
            .output(PbrOutput::new().roughness(0.5))
            .strands(
                "blades",
                StrandLayer::new()
                    .count(64)
                    .density(density)
                    .metres(1.0, 1.0),
            )
            .into_graph()
    };
    let dense = sown(&full(1.0), "blades", threads());
    assert_eq!(
        dense.len(),
        64 * 64,
        "nothing is taken away at full density"
    );
    let half = sown(&full(0.5), "blades", threads());
    let kept = share(half.len(), dense.len());
    assert!(
        (0.45..=0.55).contains(&kept),
        "half a density kept {kept} of the cells"
    );
    assert!(
        half.strands()
            .iter()
            .all(|strand| close(strand.length, 1.0)),
        "a thinned strand is a strand, at its own full length"
    );
    assert!(
        sown(&full(0.0), "blades", threads()).len() < 64,
        "nothing at zero"
    );
}

#[test]
fn a_rank_prefix_is_a_subset_of_a_looser_one() {
    // What makes a LOD honest: dropping to a quarter keeps a quarter of the
    // strands that were already there, so nothing moves when a chunk changes
    // level — it only thins.
    let set = sown(&lawn(32), "blades", threads());
    let ranks: Vec<f32> = set.strands().iter().map(|strand| strand.rank).collect();
    assert!(
        ranks.windows(2).all(|pair| pair[0] <= pair[1]),
        "the set is sorted by rank"
    );
    let quarter = set.prefix(0.25);
    let half = set.prefix(0.5);
    assert!(quarter.len() < half.len() && half.len() < set.len());
    assert_eq!(
        quarter,
        &half[..quarter.len()],
        "the tighter cut is a prefix"
    );
    assert_eq!(half, &set.strands()[..half.len()]);
    assert_eq!(set.prefix(1.1), set.strands(), "past one is everything");
    assert!(set.prefix(0.0).is_empty(), "at zero is nothing");
    // A quarter of the ranks are below a quarter, give or take the sampling.
    let kept = share(quarter.len(), set.len());
    assert!((0.2..=0.3).contains(&kept), "a rank of 0.25 kept {kept}");
}

#[test]
fn the_layers_lattice_and_its_fields_join_the_materials_repeat() {
    // A strand field is a root like a bound channel, and the lattice the roots
    // sit on is laid over the repeat by the graph, so both count.
    let material = MaterialGraph::builder("test:repeat")
        .node("coarse", Noise::value().period(3))
        .output(PbrOutput::new().roughness("coarse"))
        .strands("blades", StrandLayer::new().counts(8, 4).density("coarse"))
        .build()
        .unwrap();
    assert_eq!(material.period(), Period::Tiled { u: 24, v: 12 });
    assert_eq!(
        material.strand_port("blades", "density").map(|p| p.period),
        Some(Period::square(3))
    );
    assert_eq!(
        material
            .strand_port("blades", "direction")
            .map(|p| p.value_type),
        Some(ashlar_material::ValueType::Vec2)
    );
    assert!(material.strand("blades").is_some());
    assert_eq!(material.strands().len(), 1);
}

#[test]
fn a_field_that_does_not_tile_is_refused_at_the_node_that_broke_it() {
    let error = MaterialGraph::builder("test:free")
        .node("field", Noise::value().period(8))
        .node("free", Transform::new("field").rotate(30.0))
        .output(PbrOutput::new().roughness(0.5))
        .strands("blades", StrandLayer::new().length("free"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[free]");
    assert!(
        error.reason.contains("a strand field must tile"),
        "{}",
        error.reason
    );
}

#[test]
fn a_field_that_reaches_the_frame_is_refused_at_the_node_that_asked() {
    // The same refusal a bake makes for a world position, made here for the
    // same reason and for two more nodes: a strand is placed before there is a
    // mesh under it or a frame around it.
    let refused = |id: &str, node: ashlar_material::Node| {
        let graph = MaterialGraph::builder("test:runtime")
            .node(id, node)
            .node("field", Math::new(MathOp::Mul, id, 0.5))
            .output(PbrOutput::new().roughness(0.5))
            .strands("blades", StrandLayer::new().length("field"))
            .into_graph();
        match scattered(&graph, "blades", threads()).unwrap_err() {
            StrandError::Graph(error) => error,
            other => panic!("{other}"),
        }
    };
    for (id, node) in [
        ("place", ashlar_material::Node::from(WorldPos::new())),
        ("clock", Time::new().into()),
    ] {
        let error = refused(id, node);
        assert_eq!(error.path, format!("nodes[{id}]"));
        assert!(
            error.reason.contains("before there is either"),
            "{}",
            error.reason
        );
    }
}

#[test]
fn a_runtime_input_the_pbr_half_reads_does_not_refuse_a_scatter() {
    // A graph delivered as a `Shader` surface may read the world in its own
    // half; only what a strand field reaches is the scatter's business.
    let graph = MaterialGraph::builder("test:shader")
        .node("clock", Time::new())
        .node("field", Noise::value().period(4))
        .output(PbrOutput::new().roughness("clock").base_color("field"))
        .strands("blades", StrandLayer::new().count(4).length("field"))
        .into_graph();
    assert_eq!(sown(&graph, "blades", threads()).len(), 16);
}

#[test]
fn a_runtime_input_inside_an_instanced_graph_is_refused_by_the_path_that_names_it() {
    // Lowering inlines every node of an instance, so a clock in an unread
    // corner of a compound is a clock this scatter would have emitted.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:ticking")
            .node("clock", Time::new())
            .output(PbrOutput::new().roughness("clock"))
            .into_graph(),
    );
    let graph = MaterialGraph::builder("test:outer")
        .node(
            "inst",
            Subgraph::new("test:ticking").output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness(0.5))
        .strands("blades", StrandLayer::new().count(4).length("inst"))
        .into_graph();
    let params = no_params();
    let error = match scatter(&StrandRequest {
        graph: &graph,
        library: &library,
        params: &params,
        layer: "blades",
        field_resolution: FIELDS,
        threads: threads(),
    })
    .unwrap_err()
    {
        StrandError::Graph(error) => error,
        other => panic!("{other}"),
    };
    assert!(
        error.path.starts_with("nodes[inst].graphs[") && error.path.ends_with("nodes[clock]"),
        "{}",
        error.path
    );
}

#[test]
fn a_live_parameter_is_folded_at_the_value_the_request_bound() {
    use ashlar_material::Param;
    let graph = MaterialGraph::builder("test:lush")
        .param(Param::float("lushness", 0.25).range(0.0, 1.0).live())
        .output(PbrOutput::new().roughness(0.5))
        .strands(
            "blades",
            StrandLayer::new()
                .count(4)
                .length(Input::param("lushness"))
                .metres(1.0, 1.0),
        )
        .into_graph();
    assert!(
        sown(&graph, "blades", threads())
            .strands()
            .iter()
            .all(|strand| close(strand.length, 0.25))
    );

    let library = MaterialGraphLibrary::default();
    let params: BTreeMap<String, ParamValue> =
        [("lushness".to_owned(), ParamValue::Float(0.75))].into();
    let bound = scatter(&StrandRequest {
        graph: &graph,
        library: &library,
        params: &params,
        layer: "blades",
        field_resolution: FIELDS,
        threads: threads(),
    })
    .unwrap();
    assert!(
        bound
            .strands()
            .iter()
            .all(|strand| close(strand.length, 0.75))
    );
}

#[test]
fn a_clump_gathers_a_cells_roots_and_wraps_them_back_into_the_repeat() {
    // `VoronoiOutput::Offset` is the port's reason to exist: the vector from a
    // texel to its cell point, added to the root, pulls a lattice cell's worth
    // of blades onto one tuft.
    let graph = MaterialGraph::builder("test:tufts")
        .node(
            "towards",
            Voronoi::new().period(2).output(VoronoiOutput::Offset),
        )
        .output(PbrOutput::new().roughness(0.5))
        .strands(
            "blades",
            StrandLayer::new()
                .counts(16, 16)
                .clump("towards")
                .jitter(0.0),
        )
        .into_graph();
    let loose = sown(&lawn(16), "blades", threads());
    let tufted = sown(&graph, "blades", threads());
    assert_eq!(tufted.len(), 16 * 16);
    for strand in tufted.strands() {
        for axis in strand.root {
            assert!(
                (0.0..1.0).contains(&axis),
                "{:?} left the repeat",
                strand.root
            );
        }
    }
    // Four tufts over the repeat: with jitter off and a full offset, every root
    // of a Voronoi cell lands on that cell's own point, so there are four
    // distinct roots rather than 256.
    let mut distinct: Vec<[u32; 2]> = tufted
        .strands()
        .iter()
        .map(|strand| [strand.root[0].to_bits(), strand.root[1].to_bits()])
        .collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        4,
        "a Voronoi of four cells makes four tufts"
    );
    assert!(loose.len() > distinct.len());
}

#[test]
fn a_layer_the_graph_does_not_declare_is_refused_by_path() {
    let graph = lawn(4);
    let error = match scattered(&graph, "fur", threads()).unwrap_err() {
        StrandError::Graph(error) => error,
        other => panic!("{other}"),
    };
    assert_eq!(error.path, "strands[fur]");
    assert!(error.reason.contains("no strand layer"), "{}", error.reason);
    assert_eq!(
        scattered(&lawn(4), "blades", threads())
            .map(|set| set.layer().to_owned())
            .unwrap(),
        "blades"
    );
}

#[test]
fn a_constant_outside_its_range_is_refused_at_the_field_that_carries_it() {
    let built = |layer: StrandLayer| {
        MaterialGraph::builder("test:bad")
            .output(PbrOutput::new().roughness(0.5))
            .strands("blades", layer)
            .build()
            .unwrap_err()
    };
    assert_eq!(
        built(StrandLayer::new().count(0)).path,
        "strands[blades].count"
    );
    assert_eq!(
        built(StrandLayer::new().counts(8, 8192)).path,
        "strands[blades].count"
    );
    assert_eq!(
        built(StrandLayer::new().jitter(1.5)).path,
        "strands[blades].jitter"
    );
    assert_eq!(
        built(StrandLayer::new().segments(0)).path,
        "strands[blades].segments"
    );
    assert_eq!(
        built(StrandLayer::new().segments(ashlar_material::MAX_SEGMENTS + 1)).path,
        "strands[blades].segments"
    );
    assert_eq!(
        built(StrandLayer::new().metres(0.0, 0.004)).path,
        "strands[blades].length_metres"
    );
    assert_eq!(
        built(StrandLayer::new().root_occlusion(f32::NAN)).path,
        "strands[blades].root_occlusion"
    );
}

#[test]
fn a_field_of_the_wrong_type_is_refused_at_the_port_that_takes_it() {
    let error = MaterialGraph::builder("test:mistyped")
        .node("uv", Uv::new())
        .output(PbrOutput::new().roughness(0.5))
        .strands("blades", StrandLayer::new().density("uv"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "strands[blades].inputs[density]");
    assert!(error.reason.contains("Vec2"), "{}", error.reason);

    let unknown = MaterialGraph::builder("test:missing")
        .output(PbrOutput::new().roughness(0.5))
        .strands("blades", StrandLayer::new().length("nowhere"))
        .build()
        .unwrap_err();
    assert_eq!(unknown.path, "strands[blades].inputs[length]");
    assert!(
        unknown.reason.contains("unknown node"),
        "{}",
        unknown.reason
    );
}

#[test]
fn a_field_written_inline_is_hoisted_under_the_layer_and_the_port() {
    let material = MaterialGraph::builder("test:inline")
        .output(PbrOutput::new().roughness(0.5))
        .strands(
            "blades",
            StrandLayer::new().length(Noise::value().period(4)),
        )
        .build()
        .unwrap();
    assert!(material.port("strands.blades.length").is_some());
    assert_eq!(material.period(), Period::square(64));
}

#[test]
fn a_resolution_the_planes_cannot_be_rasterised_at_is_refused() {
    let graph = lawn(4);
    let library = MaterialGraphLibrary::default();
    let params = no_params();
    for resolution in [0, 100, 8192] {
        assert_eq!(
            scatter(&StrandRequest {
                graph: &graph,
                library: &library,
                params: &params,
                layer: "blades",
                field_resolution: resolution,
                threads: threads(),
            })
            .unwrap_err(),
            StrandError::Resolution(resolution)
        );
    }
}

#[test]
fn a_set_carries_the_constants_a_mesh_builder_cannot_derive() {
    let graph = MaterialGraph::builder("test:fur")
        .output(PbrOutput::new().roughness(0.5))
        .strands(
            "pile",
            StrandLayer::new()
                .counts(8, 4)
                .segments(5)
                .taper(0.25)
                .root_occlusion(0.6)
                .profile(StrandProfile::Fibre),
        )
        .into_graph();
    let set = sown(&graph, "pile", threads());
    assert_eq!(set.count(), [8, 4]);
    assert_eq!(set.segments(), 5);
    assert!(close(set.taper(), 0.25));
    assert!(close(set.root_occlusion(), 0.6));
    assert_eq!(set.profile(), StrandProfile::Fibre);
    assert_eq!(set.len(), 32);
    assert!(!set.is_empty());
}

#[test]
fn a_graph_written_before_strands_existed_parses_and_writes_itself_back_unchanged() {
    // The compatibility claim: `strands` is defaulted and skipped when empty,
    // so a RON file checked in before this phase reads as the graph it always
    // was and writes out without a new key in it.
    let before = r#"(
        id: "test:old",
        params: [],
        nodes: {
            "grain": Noise((
                kind: Value,
                period: (8, 8),
                seed: 0,
                octaves: 1,
                lacunarity: 2,
                persistence: 0.5,
            )),
        },
        output: (
            base_color: Const(Color((0.5, 0.5, 0.5))),
            roughness: Node("grain"),
            metallic: Const(Float(0.0)),
            occlusion: Const(Float(1.0)),
            height: None,
            normal_strength: 0.0,
            emissive: None,
            extra: {},
        ),
    )"#;
    let graph: MaterialGraph = ron::from_str(before).expect("an old graph parses");
    assert!(graph.strands.is_empty());
    let written = ron::to_string(&graph).expect("serialize");
    assert!(
        !written.contains("strands"),
        "an empty layer map is not written: {written}"
    );
    assert_eq!(
        ron::from_str::<MaterialGraph>(&written).expect("round trip"),
        graph
    );
    graph.build().expect("and it still builds");
}

#[test]
fn the_shipped_graph_library_round_trips_through_ron() {
    let library = ashlar_material::stdlib::graphs();
    let written = ron::to_string(&library).expect("serialize");
    // The key is written exactly where a graph declares a layer and nowhere
    // else, which is both halves of the compatibility claim: a library written
    // before strands existed is byte for byte the library it was, and one that
    // grew a layer says so. `library:grass` declares three.
    let declared: Vec<&str> = library
        .graphs
        .iter()
        .filter(|(_, graph)| !graph.strands.is_empty())
        .map(|(key, _)| key.as_str())
        .collect();
    assert!(declared.contains(&"library:grass"), "{declared:?}");
    assert_eq!(
        ron::from_str::<MaterialGraphLibrary>(&written).expect("round trip"),
        library
    );
}

#[test]
fn a_layer_round_trips_through_ron_with_every_field_it_carries() {
    let graph = MaterialGraph::builder("test:serde")
        .node("field", Noise::value().period(4))
        .output(PbrOutput::new().roughness("field"))
        .strands(
            "blades",
            StrandLayer::new()
                .counts(32, 16)
                .density("field")
                .length("field")
                .direction(Input::vec2([1.0, 0.0]))
                .lean(0.4)
                .bend(0.6)
                .colors([0.05, 0.12, 0.03], [0.30, 0.46, 0.12])
                .roughness(0.7)
                .jitter(0.8)
                .seed(11)
                .metres(0.18, 0.003)
                .length_variation(0.3)
                .width_variation(0.2)
                .direction_variation(0.15)
                .lean_variation(0.25)
                .segments(4)
                .taper(0.9)
                .root_occlusion(0.55)
                .profile(StrandProfile::Fibre),
        )
        .into_graph();
    let written = ron::to_string(&graph).expect("serialize");
    assert_eq!(
        ron::from_str::<MaterialGraph>(&written).expect("deserialize"),
        graph
    );
    graph.build().expect("and it builds");
}

#[test]
fn a_variation_takes_size_away_and_never_adds_any() {
    // The same rule a scatter's variations follow: the metres on the layer are
    // the longest blade rather than the average one, so a strand never outgrows
    // what the author declared.
    let with = |variation: f32| {
        MaterialGraph::builder("test:varied")
            .output(PbrOutput::new().roughness(0.5))
            .strands(
                "blades",
                StrandLayer::new()
                    .count(16)
                    .metres(1.0, 1.0)
                    .length_variation(variation)
                    .width_variation(variation),
            )
            .into_graph()
    };
    let plain = sown(&with(0.0), "blades", threads());
    assert!(
        plain
            .strands()
            .iter()
            .all(|strand| close(strand.length, 1.0) && close(strand.width, 1.0))
    );
    let varied = sown(&with(0.5), "blades", threads());
    assert!(
        varied
            .strands()
            .iter()
            .all(|strand| (0.5..=1.0).contains(&strand.length)),
        "a variation of a half never leaves a strand longer than one"
    );
    assert!(
        varied
            .strands()
            .iter()
            .any(|strand| strand.length < 0.99 && strand.width < 0.99),
        "and it does take something away"
    );
    // The two are different numbers of one strand: a long blade is not a wide
    // one, which is what the salt table is for.
    assert!(
        varied
            .strands()
            .iter()
            .any(|strand| (strand.length - strand.width).abs() > 0.05)
    );
}

#[test]
fn a_direction_reaches_a_strand_at_unit_length_and_zero_stays_zero() {
    use ashlar_material::nodes::Direction;
    let graph = MaterialGraph::builder("test:combed")
        .node("angle", Noise::value().period(4))
        .node("comb", Direction::from_angle("angle"))
        .output(PbrOutput::new().roughness("angle"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(8)
                .direction("comb")
                .direction_variation(0.4),
        )
        .into_graph();
    for strand in sown(&graph, "blades", threads()).strands() {
        let length = strand.direction[0].hypot(strand.direction[1]);
        assert!(
            close(length, 1.0),
            "{:?} is not a direction",
            strand.direction
        );
    }

    let upright = MaterialGraph::builder("test:upright")
        .output(PbrOutput::new().roughness(0.5))
        .strands(
            "blades",
            StrandLayer::new().count(8).direction_variation(1.0),
        )
        .into_graph();
    assert!(
        sown(&upright, "blades", threads())
            .strands()
            .iter()
            .all(|strand| strand.direction == [0.0, 0.0]),
        "turning nothing gives nothing"
    );
}

// --- Phase 3b: the vocabulary the research pass added. ---------------------
//
// Each of these pins one claim the grass rebuild rests on. They are together at
// the end rather than woven in above because the four promises at the top of
// this file are about the *scatter*, and these are about what one strand now
// carries.

/// A layer with the phase 3b constants dialled in, over a lattice small enough
/// to reason about.
fn tufted(count: u32, per_cell: u32, clumps: u32) -> MaterialGraph {
    MaterialGraph::builder("test:tufted")
        .output(PbrOutput::new().roughness(0.5))
        .strands(
            "blades",
            StrandLayer::new()
                .count(count)
                .per_cell(per_cell)
                .clumps(clumps, 1.0, 0.5)
                .length_variation(0.8)
                .metres(0.1, 0.004),
        )
        .into_graph()
}

#[test]
fn per_cell_multiplies_the_set_without_touching_the_period() {
    // Density that the repeat does not have to carry. `count` is a lattice the
    // material's multiple grows by; this is a salt on the seed, so three
    // strands to a cell is three times the lawn at the same period.
    let one = sown(&tufted(8, 1, 0), "blades", threads());
    let three = sown(&tufted(8, 3, 0), "blades", threads());
    assert_eq!(
        one.len() * 3,
        three.len(),
        "one lattice, three strands a cell"
    );
    assert_eq!(one.count(), three.count(), "the lattice did not move");

    let material = tufted(8, 3, 0)
        .build_in(&MaterialGraphLibrary::default())
        .unwrap();
    assert_eq!(
        material.period(),
        Period::square(8),
        "per_cell is not a period the repeat has to carry"
    );

    // And the strands of a cell are distinct: they differ in the salt their
    // hashes are taken under, so they jitter to different places.
    let roots: Vec<[u32; 2]> = three
        .strands()
        .iter()
        .map(|strand| [strand.root[0].to_bits(), strand.root[1].to_bits()])
        .collect();
    let mut unique = roots.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), roots.len(), "a cell stacked its strands");

    // Still the same set however the rows were divided.
    for count in [1, 3, 5] {
        assert_eq!(
            three,
            sown(&tufted(8, 3, 0), "blades", NonZeroUsize::new(count)),
            "per_cell changed the answer at {count} threads"
        );
    }
}

#[test]
fn the_first_strand_of_a_cell_is_the_strand_a_layer_scattered_before_per_cell() {
    // Back-compatibility in the arithmetic rather than only in the parse: index
    // zero leaves the seed alone, so an old layer's one strand a cell is the
    // strand it always was, and a layer that asks for more only adds.
    let one = sown(&tufted(8, 1, 0), "blades", threads());
    let four = sown(&tufted(8, 4, 0), "blades", threads());
    let roots: Vec<[f32; 2]> = four.strands().iter().map(|strand| strand.root).collect();
    for strand in one.strands() {
        assert!(
            roots.contains(&strand.root),
            "{:?} is not in the larger set",
            strand.root
        );
    }
}

#[test]
fn a_clump_shares_an_identity_and_pulls_its_members_tips_together() {
    let set = sown(&tufted(16, 2, 4), "blades", threads());
    assert!(!set.is_empty());

    // Every strand belongs to a clump, and there are as many identities as the
    // lattice has cells rather than as the set has strands.
    let mut ids: Vec<u32> = set
        .strands()
        .iter()
        .map(|strand| strand.clump_id.to_bits())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert!(
        (2..=16).contains(&ids.len()),
        "a 4 by 4 clump lattice gave {} identities",
        ids.len()
    );

    // The pull is towards a point, not away from one: strands sharing an
    // identity are pulled towards one place, so their root-plus-pull positions
    // are closer together than their roots are.
    let spread = |pick: fn(&ashlar_material::strands::Strand) -> [f32; 2]| {
        let members: Vec<[f32; 2]> = set
            .strands()
            .iter()
            .filter(|strand| strand.clump_id.to_bits() == ids[0])
            .map(pick)
            .collect();
        let count = members.len().max(1) as f32;
        let mean = members.iter().fold([0.0_f32; 2], |sum, point| {
            [sum[0] + point[0] / count, sum[1] + point[1] / count]
        });
        members
            .iter()
            .map(|point| (point[0] - mean[0]).hypot(point[1] - mean[1]))
            .sum::<f32>()
            / count
    };
    let roots = spread(|strand| strand.root);
    let tips = spread(|strand| {
        [
            strand.root[0] + strand.clump_pull[0],
            strand.root[1] + strand.clump_pull[1],
        ]
    });
    assert!(
        tips < roots,
        "tips at {tips} are no closer than roots at {roots}"
    );

    // A layer that declares no clump lattice carries none of it.
    for strand in sown(&tufted(16, 2, 0), "blades", threads()).strands() {
        assert_eq!(strand.clump_id, 0.0);
        assert_eq!(strand.clump_pull, [0.0, 0.0]);
    }
}

#[test]
fn a_shared_clump_makes_its_members_agree_and_an_unshared_one_does_not() {
    // The correlation length Blender splits its child roughness by: at a share
    // of one the blades of a tuft are the same length, at zero they are
    // independent. Measured as the spread of lengths *within* one clump.
    let within = |share: f32| {
        let graph = MaterialGraph::builder("test:share")
            .output(PbrOutput::new().roughness(0.5))
            .strands(
                "blades",
                StrandLayer::new()
                    .count(16)
                    .per_cell(2)
                    .clumps(4, share, 0.0)
                    .length_variation(0.9)
                    .metres(0.1, 0.004),
            )
            .into_graph();
        let set = sown(&graph, "blades", threads());
        let first = set.strands()[0].clump_id.to_bits();
        let lengths: Vec<f32> = set
            .strands()
            .iter()
            .filter(|strand| strand.clump_id.to_bits() == first)
            .map(|strand| strand.length)
            .collect();
        let count = lengths.len().max(1) as f32;
        let mean = lengths.iter().sum::<f32>() / count;
        lengths.iter().map(|l| (l - mean).abs()).sum::<f32>() / count
    };
    let shared = within(1.0);
    let independent = within(0.0);
    assert!(
        shared < independent,
        "a shared clump spread lengths by {shared}, an unshared one by {independent}"
    );
}

#[test]
fn facing_and_height_offset_are_per_strand_bounded_and_deterministic() {
    let plain = sown(&tufted(8, 1, 0), "blades", threads());
    assert!(
        plain.strands().iter().all(|strand| strand.facing == 0.0),
        "a layer that asks for no facing gets none"
    );
    assert!(
        plain
            .strands()
            .iter()
            .all(|strand| strand.height_offset == 0.0),
        "and no sink"
    );

    let graph = MaterialGraph::builder("test:turned")
        .output(PbrOutput::new().roughness(0.5))
        .strands(
            "blades",
            StrandLayer::new()
                .count(12)
                .facing_variation(1.0)
                .height_offset(0.05)
                .metres(0.1, 0.004),
        )
        .into_graph();
    let set = sown(&graph, "blades", threads());
    let quarter = std::f32::consts::FRAC_PI_2;
    assert!(
        set.strands()
            .iter()
            .all(|strand| strand.facing.abs() <= quarter + 1e-6),
        "a full facing variation is a quarter turn either way"
    );
    assert!(
        set.strands()
            .iter()
            .all(|strand| (0.0..=0.05 + 1e-6).contains(&strand.height_offset)),
        "a sink never lifts a root and never exceeds what was asked for"
    );
    // Both vary, and neither depends on the division of the rows.
    let turned: Vec<u32> = set.strands().iter().map(|s| s.facing.to_bits()).collect();
    let mut unique = turned.clone();
    unique.sort_unstable();
    unique.dedup();
    assert!(unique.len() > turned.len() / 2, "facing barely varies");
    assert_eq!(set, sown(&graph, "blades", NonZeroUsize::new(5)));
}

#[test]
fn a_layer_written_before_phase_3b_parses_and_scatters_what_it_always_did() {
    // Serde back-compatibility, and the arithmetic behind it: a RON file that
    // names none of the new constants reads as the defaults, and those defaults
    // are exactly the behaviour the layer had.
    let old = r"(
        density: Const(Float(1.0)),
        count: (16, 16),
        length_metres: 0.1,
        width_metres: 0.004,
    )";
    let layer: StrandLayer = ron::from_str(old).expect("an old layer still parses");
    assert_eq!(layer.per_cell, 1);
    assert_eq!(layer.midpoint, 0.0);
    assert_eq!(layer.facing_variation, 0.0);
    assert_eq!(layer.height_offset_metres, 0.0);
    assert_eq!(layer.clump_count, [0, 0]);
    assert_eq!(layer.clump_share, 0.0);
    assert_eq!(layer.clump_tips, 0.0);
    assert_eq!(layer.clump_tint, 0.0);

    let graph = MaterialGraph::builder("test:old")
        .output(PbrOutput::new().roughness(0.5))
        .strands("blades", layer)
        .into_graph();
    let set = sown(&graph, "blades", threads());
    assert_eq!(set.len(), 16 * 16, "density one keeps every cell");
    assert!(
        set.strands()
            .iter()
            .all(|s| s.facing == 0.0 && s.height_offset == 0.0 && s.clump_id == 0.0),
        "an old layer carries none of the new per-strand facts"
    );
}

#[test]
fn a_width_profile_puts_the_widest_point_where_the_midpoint_says() {
    use ashlar_material::strands::width_at;
    // The one definition the mesh builder and the splat both read. At a
    // midpoint of zero it is the plain taper a layer described before this
    // existed, which is what keeps every old layer's silhouette.
    for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
        assert!(close(width_at(t, 0.8, 0.0), 1.0 - 0.8 * t));
    }
    // Above zero the blade narrows towards the root as well, and the widest
    // point is the midpoint itself.
    let midpoint = 0.35;
    let widest = width_at(midpoint, 0.9, midpoint);
    assert!(close(widest, 1.0));
    for t in [0.0, 0.1, 0.6, 1.0] {
        assert!(
            width_at(t, 0.9, midpoint) < widest,
            "{t} is no narrower than the midpoint"
        );
    }
    // And a taper steep enough to run past zero ends in a point rather than
    // turning the blade inside out.
    assert!(width_at(1.0, 1.0, 0.5) >= 0.0);
}

#[test]
fn a_clump_owns_the_bend_and_the_facing_as_well_as_the_length() {
    // The promoted clump item: "that clump controls the blade's height,
    // direction, colour, bend". Every one of those runs through the same
    // correlation-length mixing, so what this pins is that bend and facing are
    // in it and not only length and lean.
    let spread = |share: f32, pick: fn(&ashlar_material::strands::Strand) -> f32| {
        let graph = MaterialGraph::builder("test:owned")
            .output(PbrOutput::new().roughness(0.5))
            .strands(
                "blades",
                StrandLayer::new()
                    .count(16)
                    .per_cell(2)
                    .clumps(4, share, 0.0)
                    .bend(0.5)
                    .bend_variation(0.9)
                    .facing_variation(1.0)
                    .metres(0.1, 0.004),
            )
            .into_graph();
        let set = sown(&graph, "blades", threads());
        let first = set.strands()[0].clump_id.to_bits();
        let members: Vec<f32> = set
            .strands()
            .iter()
            .filter(|strand| strand.clump_id.to_bits() == first)
            .map(pick)
            .collect();
        let count = members.len().max(1) as f32;
        let mean = members.iter().sum::<f32>() / count;
        members.iter().map(|v| (v - mean).abs()).sum::<f32>() / count
    };
    for (what, pick) in [
        (
            "bend",
            (|s: &ashlar_material::strands::Strand| s.bend) as fn(&_) -> f32,
        ),
        ("facing", |s: &ashlar_material::strands::Strand| s.facing),
    ] {
        let shared = spread(1.0, pick);
        let independent = spread(0.0, pick);
        assert!(
            shared < independent,
            "{what} varies by {shared} inside a shared clump and {independent} inside an \
             unshared one, so the clump does not own it"
        );
    }
}

#[test]
fn a_clump_distance_runs_from_its_centre_to_the_edge_of_its_cell() {
    let set = sown(&tufted(24, 2, 4), "blades", threads());
    assert!(
        set.strands()
            .iter()
            .all(|strand| (0.0..=1.0).contains(&strand.clump_distance)),
        "a clump distance is a fraction of the cell's own half-diagonal"
    );
    // It is a real spread rather than a constant: some blades stand near their
    // tuft's centre and some out at its edge, which is the axis a curve-shaped
    // clump operator needs.
    let near = set
        .strands()
        .iter()
        .filter(|strand| strand.clump_distance < 0.3)
        .count();
    let far = set
        .strands()
        .iter()
        .filter(|strand| strand.clump_distance > 0.6)
        .count();
    assert!(
        near > 0 && far > 0,
        "{near} near the centre, {far} at the edge"
    );

    // And a layer with no clump lattice carries none of it.
    assert!(
        sown(&tufted(16, 2, 0), "blades", threads())
            .strands()
            .iter()
            .all(|strand| strand.clump_distance == 0.0)
    );
}
