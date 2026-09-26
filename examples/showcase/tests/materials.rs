//! The showcase's own graphs: the four compiled surfaces over the default
//! library. The library's graphs are tested where they live, in
//! `ashlar-material`; what is pinned here is what the showcase adds.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test, where a failure is the test failing"
)]
use std::collections::BTreeMap;

use ashlar_material::{
    Material, Period,
    bake::BakeRequest,
    ir::{Filter, Target},
    partition::{MAX_BOUND_TEXTURES, OutputSource, partition},
};
use ashlar_showcase::{library, materials};

const THREADS: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(4);

/// Every parameter the graph declares, live.
fn everything(material: &Material) -> Target {
    Target::Shader {
        live: material
            .graph()
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect(),
    }
}

/// Nothing live at all.
fn nothing() -> Target {
    Target::Shader { live: Vec::new() }
}

#[test]
fn every_showcase_graph_validates_tiles_and_is_compiled() {
    let graphs = materials::graphs();
    graphs.check().expect("the showcase's graphs build");
    let built = graphs.build_all().expect("the showcase's graphs build");
    let own: Vec<&String> = built
        .keys()
        .filter(|key| key.starts_with("showcase:"))
        .collect();
    assert_eq!(own.len(), materials::compiled().len());
    for key in own {
        assert!(materials::compiled().contains(&key.as_str()), "{key}");
        let material = &built[key];
        assert!(
            matches!(material.period(), Period::Tiled { .. }),
            "{key} does not tile"
        );
        assert!(
            material.warnings().is_empty(),
            "{key}: {:?}",
            material.warnings()
        );
        let lattice = material.finest_lattice();
        assert!(lattice[0] <= 512 && lattice[1] <= 512, "{key}: {lattice:?}");
    }
    // A light strip is forty-five millimetres of flat acrylic, so it binds no
    // height; the wet concrete is the library's cast panel and keeps its relief.
    assert!(built["showcase:strip"].output_port("height").is_none());
    assert!(
        built["showcase:concrete-wet"]
            .output_port("height")
            .is_some()
    );
}

#[test]
fn every_definition_names_a_graph_the_library_holds() {
    let graphs = materials::graphs();
    for (key, definition) in library::materials().materials {
        if let Some(graph) = definition.surface.graph() {
            assert!(graphs.get(graph).is_some(), "{key} names {graph}");
        }
    }
}

/// The one graph the showcase ships that asks the world a question, and what it
/// costs to ask it.
///
/// Three claims, and each is a different half of why `showcase:brick-weathered`
/// is in [`materials::compiled`]. It **cannot**
/// be baked, because a bake refuses a world-space input by path rather than
/// folding it to zero. It **does** partition, at the resolution the adapter
/// compiles a `Surface::Shader` at by default, inside the eight bound images one
/// Bevy material extension declares. And its live `wet` survives the split: the
/// whole reason `weathering:moisture` is the compound to reach for when the
/// weather moves is that every filter in it is pointwise, so nothing the live
/// value reaches has to become a plane and `frozen` stays empty.
///
/// The counts are asserted as bounds rather than as equalities. An exact op
/// count is a golden hash without the value of one — it fails on any
/// improvement to the lowering — while "it fits in the bind group" and "nothing
/// froze" are the properties a wall actually depends on. The measured numbers of
/// the day are in the graph's own doc comment, which is where a reader wanting
/// them will look.
#[test]
fn the_weathered_brick_refuses_a_bake_partitions_and_freezes_nothing() {
    let graphs = materials::graphs();
    let key = "showcase:brick-weathered";
    let material = graphs.build(key).expect("the weathered brick builds");

    // A world normal is not a field over UV, and the bake says so at the node.
    let refusal = ashlar_material::bake::bake(&BakeRequest {
        graph: graphs.get(key).expect("the showcase ships its graphs"),
        library: &graphs,
        params: &BTreeMap::new(),
        resolution: 512,
        mips: false,
        threads: THREADS,
    })
    .expect_err("a world mask cannot be baked");
    let refusal = refusal.to_string();
    assert!(
        refusal.contains("nodes[upward]") && refusal.contains("world normal"),
        "the refusal should name the WorldMask node: {refusal}"
    );

    let split =
        ashlar_material::partition::partition(&material, &Target::shader_for(&material), 512)
            .expect("the weathered brick partitions");
    let report = split.report();
    assert!(
        !split.is_static(),
        "a graph with a live parameter and a world mask is not static: {report}"
    );
    assert_eq!(split.live(), ["wet"], "{report}");
    assert!(
        report.frozen.is_empty(),
        "moisture is pointwise throughout, so a live `wet` freezes nothing: {report}"
    );
    assert_eq!(report.params, 1, "one uniform: {report}");
    assert!(
        split.images() <= 8,
        "one Bevy material extension declares eight texture and sampler pairs: {report}"
    );
    // The height is the brick's own and so is a bound plane, which is what
    // leaves the derived normal static — and a static normal map is what carries
    // the footprint coherence a compiled roughness widens with.
    assert_eq!(report.taps, 0, "{report}");
    assert!(
        report.widens,
        "the roughness should widen with distance the way the baked brick's does: {report}"
    );
    assert!(
        matches!(material.period(), Period::Tiled { .. }) && material.warnings().is_empty(),
        "{key} does not tile cleanly: {:?}",
        material.warnings()
    );
}

#[test]
fn the_light_strip_is_a_pulse_over_bound_textures_whatever_is_live() {
    // The one shipped graph that cannot be baked, and the reason a fragment is
    // worth compiling at all. Its parameter is the pulse *rate*, which is a
    // decision about what the light is rather than something that moves, so it
    // folds; what is left runtime is the clock and the handful of instructions
    // between it and the emissive slot, over a diffuser that is entirely
    // static. That is true whether or not the rate is live, which is the point:
    // nothing about `Time` is a parameter, so nothing a parameter does can
    // remove it.
    let library = materials::graphs();
    let material = &library.build_all().unwrap()["showcase:strip"];
    for target in [nothing(), everything(material)] {
        let split = partition(material, &target, 512).unwrap();
        let report = split.report();
        assert_ne!(
            report.verdict, "no live inputs: bake this",
            "a clock is a live input: {report}"
        );
        // The emissive is the only slot the clock reaches; everything else the
        // graph binds is a texture read.
        assert!(
            split
                .output("emissive")
                .is_some_and(OutputSource::is_runtime),
            "{report}"
        );
        for port in ["base_color", "roughness"] {
            assert!(
                split
                    .output(port)
                    .is_some_and(|source| !source.is_runtime()),
                "{port} moved into the fragment: {report}"
            );
        }
        // A sine, a multiply into radians, a scale, an add and a multiply into
        // the bound emissive, plus the clock itself. Small enough to write down
        // is the claim; the exact number is the emitter's business.
        assert!(report.ops > 0 && report.ops <= 16, "{report}");
        assert!(report.textures <= 4, "{report}");
        // No height, so no derived normal and no plane for one.
        assert!(split.output("normal").is_none(), "{report}");
        assert!(report.taps == 0, "{report}");
    }
}

#[test]
fn the_wet_concrete_gates_its_wetness_on_the_world_and_cannot_be_baked() {
    // Another graph a texture cannot hold. It instances `library:formed-concrete` — and multiplies its live `wetness` by two world
    // masks: which way the face points, and how high it stands. So the same
    // material is wet on a coping and dry on the wall under it, which was a
    // slot somebody bound by hand until now.
    let library = materials::graphs();
    let material = &library.build_all().unwrap()["showcase:concrete-wet"];

    // A bake refuses it, by node path, and says what to do instead.
    let error = ashlar_material::ir::lower(material, Target::Bake)
        .expect_err("a world-space graph is not a texture");
    assert!(error.path.starts_with("nodes["), "{error}");
    assert!(error.reason.contains("Shader surface"), "{error}");

    let split = partition(material, &nothing(), 512).unwrap();
    let report = split.report();
    assert_eq!(split.live(), ["wetness"], "{report}");
    assert!(split.frozen().is_empty(), "{report}");
    // The two slots water changes are computed; the three it does not are
    // texture reads straight into their PBR slots, exactly as they were when
    // the wetness was ungated.
    for port in ["base_color", "roughness"] {
        assert!(
            split.output(port).is_some_and(OutputSource::is_runtime),
            "{port} is what the water changes: {report}"
        );
    }
    for port in ["occlusion", "height", "normal"] {
        assert!(
            matches!(split.output(port), Some(OutputSource::Texture(_))),
            "{port} moved into the fragment: {report}"
        );
    }
    // The relief survives the indirection: the normal is a bound map derived
    // from the height the subgraph handed up, not a flat mesh normal.
    assert!(
        split
            .textures()
            .iter()
            .any(|texture| matches!(texture.filter, Filter::Normal { .. })),
        "the wet concrete lost the normal its dry twin has"
    );
    assert!(report.textures <= MAX_BOUND_TEXTURES, "{report}");
    // Twenty-five instructions where the ungated wetness was eleven: the two
    // masks are a lane, a ramp and their edges each, and the gate is one more
    // multiply into the uniform. Everything above them — four noises, the pore
    // mask, the panel joint, the sRGB decode and a horizon march — is still on
    // the static side, in the seven bound textures.
    assert!(report.ops > 0 && report.ops <= 32, "{report}");
}
