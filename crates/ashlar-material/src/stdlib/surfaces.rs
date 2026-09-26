//! Shared substances and the default, physically sized building surfaces.
//!
//! Heights use a 20 mm range unless a builder says otherwise. A substance
//! exports its small-scale relief; the enclosing material adds its layout.

use crate::{
    BlendMode, BrickOutput, Channel, Input, MaterialGraph, MaterialGraphBuilder, MathOp, Param,
    ParamValue, PbrOutput, SurfaceOutput,
    nodes::{
        Blend, Bricks, Clamp, Combine2, Decompose, GraphInput, IntensityWarp, Invert, Levels, Math,
        Mix, Noise, OcclusionFromHeight, Subgraph, Transform, Uv, Voronoi, VoronoiOutput, Warp,
    },
};

pub(super) fn m(op: MathOp, a: impl Into<Input>, b: impl Into<Input>) -> Math {
    Math::new(op, a, b)
}

pub(super) fn p(name: &str) -> Input {
    Input::param(name)
}

pub(super) fn remap(input: impl Into<Input>, low: f32, high: f32) -> Levels {
    Levels::new(input).out_range(low, high)
}

/// The PBR channels shared by a substrate instance.
pub(super) const CHANNELS: [SurfaceOutput; 4] = [
    SurfaceOutput::BaseColor,
    SurfaceOutput::Roughness,
    SurfaceOutput::Metallic,
    SurfaceOutput::Height,
];

pub(super) fn finish(
    g: MaterialGraphBuilder,
    output: PbrOutput,
    tile: f32,
    depth: f32,
) -> MaterialGraph {
    g.node("physical_height", m(MathOp::Mul, "height", depth / tile))
        .node(
            "ao",
            OcclusionFromHeight::new("physical_height").radius(0.012),
        )
        .tile_metres([tile; 2])
        .output(
            output
                .height("height")
                .occlusion("ao")
                .normal_strength(depth / tile),
        )
        .into_graph()
}

/// Rain on a finished ground surface, gated by the caller's `wet` parameter.
///
/// `weathering:moisture` over the dry colour and roughness, deciding where
/// water stands from `pond` rather than from the relief alone: a road holds
/// puddles in its ruts and a broad sag, not in the gaps between stones, so the
/// caller says where the ground is low at the scale water cares about. Where
/// it pools the height is raised to `water`, a flat surface just over the
/// stone tops, so the derived normal goes flat under a near-mirror finish.
///
/// Leaves `rain.base_color`, `rain.roughness` and `height`. At a `wet` of zero
/// every one of them is the dry field bit for bit: the absorption is `exp2` of
/// zero, and every mix the compound and this make is at a weight of zero.
pub(super) fn rained_on(
    g: MaterialGraphBuilder,
    dry: [&str; 3],
    pond: impl Into<Input>,
    level: f32,
    water: f32,
) -> MaterialGraphBuilder {
    let [color, roughness, height] = dry;
    g.layer(
        "rain",
        Subgraph::new("weathering:moisture")
            .input("height", pond)
            .input("base_color", color)
            .input("roughness", roughness)
            .input("wet", p("wet"))
            .param("level", ParamValue::Float(level))
            .param("softness", ParamValue::Float(0.04))
            .param("darkening", ParamValue::Float(6.0))
            .param("porosity", ParamValue::Float(1.0)),
        &[
            SurfaceOutput::BaseColor,
            SurfaceOutput::Roughness,
            SurfaceOutput::Extra("wet_mask".into()),
        ],
    )
    .node(
        "height",
        Mix::new(height, m(MathOp::Max, height, water), "rain.wet_mask"),
    )
}

/// Cast cement paste, fines, sparse air holes and vertical pour variation.
///
/// A two metre repeat, with height measured in a 20 mm range. `unit_id` comes
/// from the caller's layout and varies each slab as a whole. Air-hole presence
/// and radius share one cellular lattice; the fine grain does not place holes.
/// `pores` controls occurrence, and `pore_depth` controls depth independently.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn cast_cement() -> MaterialGraph {
    use MathOp::{Add, Mul, Step, Sub};
    let holes = Voronoi::new().period(64).seed(137);
    let g = MaterialGraph::builder("substances:cast-cement")
        .param(Param::color("color", [0.22, 0.217, 0.202]))
        .param(Param::float("casting", 1.0).range(0.0, 1.0))
        .param(Param::float("pores", 0.20).range(0.0, 1.0))
        .param(Param::float("pore_depth", 0.07).range(0.0, 0.15))
        .node("unit", GraphInput::float("unit_id", 0.5))
        .node("paste", Noise::perlin().period(8).octaves(3).seed(19))
        .node("pour", Noise::perlin().periods(32, 4).octaves(2).seed(31))
        .node("fines", Noise::perlin().period(128).octaves(2).seed(43))
        .node(
            "grit",
            Blend::new(BlendMode::Dissolve, 0.0, 1.0).opacity(0.4),
        )
        .node("hole_id", holes.output(VoronoiOutput::Cell))
        .node("hole_distance", holes.output(VoronoiOutput::Distance))
        .node(
            "hole_size",
            Math::unary(MathOp::Fract, m(Mul, "hole_id", 17.17)),
        )
        .node("hole_radius", remap("hole_size", 0.055, 0.23))
        .node(
            "hole_profile",
            Clamp::new(m(
                MathOp::Div,
                m(Sub, "hole_radius", "hole_distance"),
                "hole_radius",
            )),
        )
        .node("hole_present", m(Step, p("pores"), "hole_id"))
        .node("pinholes", m(Mul, "hole_profile", "hole_present"))
        .node("scar_distance", Voronoi::new().period(32).seed(211))
        .node(
            "scar_id",
            Voronoi::new()
                .period(32)
                .seed(211)
                .output(VoronoiOutput::Cell),
        )
        .node(
            "scar_edge",
            IntensityWarp::new("scar_distance", remap("fines", -0.004, 0.004)).amount(1.0),
        )
        .node(
            "scar_profile",
            Invert::new(
                Levels::new(m(Add, "scar_edge", remap("fines", -0.10, 0.10))).in_range(0.055, 0.11),
            ),
        )
        .node(
            "scars",
            m(
                Mul,
                "scar_profile",
                m(Step, m(Mul, p("pores"), 0.6), "scar_id"),
            ),
        )
        .node("pores", m(MathOp::Max, "pinholes", "scars"))
        .node(
            "tone",
            m(
                Add,
                m(
                    Add,
                    remap("paste", 0.78, 1.1),
                    m(Mul, remap("pour", -0.4, 0.4), p("casting")),
                ),
                remap("unit", -0.08, 0.08),
            ),
        )
        .node("paste_color", m(Mul, p("color"), "tone"))
        .node(
            "color",
            m(
                Mul,
                m(
                    Mul,
                    m(Mul, "paste_color", remap("fines", 0.85, 1.1)),
                    remap("grit", 0.965, 1.035),
                ),
                remap("pores", 1.0, 0.82),
            ),
        )
        .node(
            "roughness",
            Clamp::new(m(Add, remap("fines", 0.76, 0.9), m(Mul, "pores", 0.07))),
        )
        .node(
            "skin",
            m(
                Add,
                remap("paste", 0.46, 0.50),
                m(
                    Add,
                    m(Mul, remap("pour", -0.022, 0.022), p("casting")),
                    m(
                        Add,
                        remap("fines", -0.018, 0.018),
                        remap("grit", -0.0015, 0.0015),
                    ),
                ),
            ),
        )
        .node("height", m(Sub, "skin", m(Mul, "pores", p("pore_depth"))));
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0)
            .extra("pores", "pores"),
        2.0,
        0.02,
    )
}

/// Formed concrete: cast cement with metre-wide panels and recessed tie holes.
///
/// The Concrete031 reference establishes grey paste, vertical casting marks,
/// sparse pinholes and fine panel seams. Tie holes are a separate control so
/// the same substrate serves liners and cut faces without a formwork pattern.
/// The default tie recess is 2.4 mm and the seam is 1.6 mm in a 20 mm range.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn formed_concrete() -> MaterialGraph {
    use MathOp::{Add, Min, Mul, Sub};
    let g = MaterialGraph::builder("library:formed-concrete")
        .param(Param::color("color", [0.165, 0.162, 0.15]))
        .param(Param::float("seams", 1.0).range(0.0, 1.0))
        .param(Param::float("ties", 0.55).range(0.0, 1.0))
        .node(
            "panel_id",
            Bricks::new()
                .rows(2)
                .columns(2)
                .offset(0.0)
                .seed(71)
                .output(BrickOutput::Id),
        )
        .node("panel_variation", Mix::new(0.5, "panel_id", p("seams")))
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("x", Math::unary(MathOp::Fract, m(Mul, "u", 2.0)))
        .node("y", Math::unary(MathOp::Fract, m(Mul, "v", 2.0)))
        .node(
            "edge",
            m(
                Min,
                m(Min, "x", Invert::new("x")),
                m(Min, "y", Invert::new("y")),
            ),
        )
        .node(
            "seam",
            m(
                Mul,
                Invert::new(Levels::new("edge").in_range(0.0, 0.003)),
                p("seams"),
            ),
        )
        .node("tie_centres", Voronoi::new().period(4).jitter(0.0))
        .node(
            "tie_shifted",
            Transform::new("tie_centres").translate(0.035, 0.0),
        )
        .node(
            "tie",
            m(
                Mul,
                Invert::new(Levels::new("tie_shifted").in_range(0.024, 0.046)),
                p("ties"),
            ),
        )
        .layer(
            "cement",
            Subgraph::new("substances:cast-cement")
                .input("unit_id", "panel_variation")
                .param("casting", ParamValue::Float(0.25)),
            &CHANNELS,
        )
        // Cloudy tone at three scales: broad patches a panel wide, blotches a
        // hand wide, and short vertical smears where the paste slid down the
        // form face. None of them touches the height, so the formwork
        // controls stay the only thing that recesses the cast surface.
        .node("cloud", Noise::perlin().period(4).octaves(4).seed(907))
        .node(
            "blotch",
            Noise::perlin().periods(16, 8).octaves(4).seed(911),
        )
        .node(
            "smear",
            Noise::perlin().periods(128, 16).octaves(2).seed(919),
        )
        .node("stain", Noise::perlin().period(8).octaves(4).seed(929))
        .node("dark_blotch", Levels::new("blotch").in_range(0.52, 0.72))
        .node("olive_stain", Levels::new("stain").in_range(0.6, 0.78))
        .node(
            "mottle",
            m(
                Mul,
                m(Mul, remap("cloud", 0.74, 1.2), remap("smear", 0.86, 1.1)),
                m(
                    Mul,
                    Invert::new(m(Mul, "dark_blotch", 0.28)),
                    remap("panel_variation", 0.93, 1.06),
                ),
            ),
        )
        .node(
            "tint",
            m(MathOp::Div, p("color"), Input::color([0.22, 0.217, 0.202])),
        )
        .node(
            "stained",
            Mix::new(
                m(Mul, m(Mul, "cement.base_color", "tint"), "mottle"),
                m(
                    Mul,
                    m(Mul, "cement.base_color", "tint"),
                    Input::color([0.78, 0.8, 0.62]),
                ),
                m(Mul, "olive_stain", 0.6),
            ),
        )
        .node(
            "color",
            m(
                Mul,
                "stained",
                m(Sub, 1.0, m(Add, m(Mul, "seam", 0.12), m(Mul, "tie", 0.12))),
            ),
        )
        .node(
            "roughness",
            Clamp::new(m(
                Add,
                m(Sub, "cement.roughness", 0.12),
                m(
                    Add,
                    remap("cloud", -0.06, 0.06),
                    m(Mul, "dark_blotch", -0.08),
                ),
            )),
        )
        .node(
            "height",
            m(
                Sub,
                "cement.height",
                m(Add, m(Mul, "seam", 0.08), m(Mul, "tie", 0.12)),
            ),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0)
            .extra("seams", "seam")
            .extra("ties", "tie")
            .extra("unit_id", "panel_id"),
        2.0,
        0.02,
    )
}

/// Half-metre cast paving slabs, a fine bevel and moss confined to the joints.
///
/// The `PavingStones136` reference has pale buff faces and narrow, darker joints.
/// Both the slab and the grout instance cast cement; their ownership, per-slab
/// tone and moss habitat all come from `layouts:slab-lattice`. `tint`
/// multiplies slab and grout alike, so a city can lay the same paving dark;
/// `wet` is rain, standing in the joints and on the low side of a settled
/// slab (see `rained_on`). At their defaults both leave the bake exactly as
/// it was.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn paving_slabs() -> MaterialGraph {
    use MathOp::{Add, Fract, Mul, Sub};
    let g = MaterialGraph::builder("library:paving-slabs")
        .param(Param::float("moss", 0.5).range(0.0, 1.0))
        .param(Param::color("tint", [1.0, 1.0, 1.0]))
        .param(Param::float("wet", 0.0).range(0.0, 1.0))
        .layer(
            "layout",
            Subgraph::new("layouts:slab-lattice")
                .param("joint_width", ParamValue::Float(0.007))
                .param("bevel", ParamValue::Float(0.005))
                .param("irregularity", ParamValue::Float(0.8)),
            &[
                SurfaceOutput::Extra("unit_id".into()),
                SurfaceOutput::Extra("mask".into()),
                SurfaceOutput::Extra("joint_depth".into()),
                SurfaceOutput::Extra("edge_distance".into()),
                SurfaceOutput::Extra("local_u".into()),
                SurfaceOutput::Extra("local_v".into()),
            ],
        )
        .layer(
            "cement",
            Subgraph::new("substances:cast-cement")
                .input("unit_id", "layout.unit_id")
                .param("color", ParamValue::Color([0.41, 0.35, 0.17]))
                .param("pores", ParamValue::Float(0.1))
                .param("casting", ParamValue::Float(0.0)),
            &CHANNELS,
        )
        .layer(
            "grout",
            Subgraph::new("substances:cast-cement")
                .param("color", ParamValue::Color([0.075, 0.066, 0.042]))
                .param("casting", ParamValue::Float(0.0)),
            &CHANNELS,
        )
        // Each slab settles on its own: two more hashes of its id tilt it
        // about both axes and a third sinks it, by a couple of millimetres.
        .node(
            "tilt_u",
            m(
                Sub,
                Math::unary(Fract, m(Mul, "layout.unit_id", 17.31)),
                0.5,
            ),
        )
        .node(
            "tilt_v",
            m(
                Sub,
                Math::unary(Fract, m(Mul, "layout.unit_id", 71.93)),
                0.5,
            ),
        )
        .node(
            "settle",
            m(
                Add,
                m(
                    Add,
                    m(Mul, "tilt_u", m(Sub, "layout.local_u", 0.5)),
                    m(Mul, "tilt_v", m(Sub, "layout.local_v", 0.5)),
                ),
                remap("layout.unit_id", -0.035, 0.02),
            ),
        )
        // A worn, rounded arris rather than a chamfer.
        .node("arris", m(Mul, "layout.mask", m(Sub, 2.0, "layout.mask")))
        .node(
            "face_height",
            m(
                Add,
                0.62,
                m(
                    Add,
                    m(Mul, m(Sub, "cement.height", 0.48), 0.6),
                    m(Mul, "settle", 0.3),
                ),
            ),
        )
        .node(
            "laid_height",
            Mix::new(m(Mul, "grout.height", 0.55), "face_height", "arris"),
        )
        // Traffic and weather: broad stains, darker dirt towards each joint
        // where water runs off, and a speckle of exposed aggregate.
        .node("stain", Noise::perlin().period(8).octaves(4).seed(941))
        .node("wet_edge", Noise::perlin().period(32).octaves(2).seed(947))
        .node(
            "aggregate",
            Blend::new(BlendMode::Dissolve, 0.0, 1.0).opacity(0.28),
        )
        .node(
            "edge_dirt",
            m(
                Mul,
                Invert::new(Levels::new("layout.edge_distance").in_range(0.0, 0.016)),
                remap("wet_edge", 0.2, 0.55),
            ),
        )
        .node(
            "face_tone",
            m(
                Mul,
                m(
                    Mul,
                    remap("layout.unit_id", 0.86, 1.09),
                    remap(Levels::new("stain").in_range(0.35, 0.8), 1.04, 0.84),
                ),
                m(
                    Mul,
                    Invert::new("edge_dirt"),
                    remap("aggregate", 0.88, 1.07),
                ),
            ),
        )
        .node(
            "face_color",
            m(
                Mul,
                m(Mul, "cement.base_color", "face_tone"),
                Mix::new(
                    Input::color([1.0, 1.0, 1.0]),
                    Input::color([0.9, 0.95, 0.78]),
                    "edge_dirt",
                ),
            ),
        )
        .node(
            "laid_color",
            m(
                Mul,
                Mix::new("grout.base_color", "face_color", "layout.mask"),
                p("tint"),
            ),
        )
        .node(
            "laid_roughness",
            Mix::new(
                0.95,
                Clamp::new(m(Sub, "cement.roughness", remap("stain", 0.0, 0.1))),
                "layout.mask",
            ),
        )
        .layer(
            "moss",
            Subgraph::new("weathering:moss")
                .input("height", "laid_height")
                .input("base_color", "laid_color")
                .input("roughness", "laid_roughness")
                .input("metallic", 0.0)
                .input("bias", "layout.joint_depth")
                .input("shelter", 1.0)
                .input("coverage", p("moss"))
                .param("amount", ParamValue::Float(1.0))
                .param("dark_color", ParamValue::Color([0.032, 0.03, 0.01]))
                .param("light_color", ParamValue::Color([0.1, 0.11, 0.035]))
                .param("depth", ParamValue::Float(0.08)),
            &CHANNELS,
        )
        // Where rain stands on paving: in the joints, and on the low side of
        // a slab that has settled, over a broad sag in the whole pavement.
        .node(
            "pond",
            m(
                Add,
                m(Mul, "moss.height", 0.75),
                m(Mul, Noise::perlin().period(4).octaves(2).seed(953), 0.35),
            ),
        );
    let g = rained_on(
        g,
        ["moss.base_color", "moss.roughness", "moss.height"],
        "pond",
        0.6,
        0.66,
    );
    finish(
        g,
        PbrOutput::new()
            .base_color("rain.base_color")
            .roughness("rain.roughness")
            .metallic(0.0)
            .extra("unit_id", "layout.unit_id")
            .extra("mask", "layout.mask")
            .extra("joint_depth", "layout.joint_depth"),
        2.0,
        0.02,
    )
}

/// Pitted cut limestone, with fine bedding and clustered solution voids.
///
/// `unit_id` varies a slab's overall tone and how many voids it carries.
/// Voids gather in colonies, as they do in travertine and shelly limestone:
/// a dark hole with a greyed halo, the hole a recess, the halo a rougher skin.
/// `pitting` scales those recesses independently of the slab's layout, and
/// `mineral_amount` how many colonies there are. The halo is the `minerals`
/// extra.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn cut_limestone() -> MaterialGraph {
    use MathOp::{Add, Div, Fract, Max, Mul, Step, Sub};
    // Voids are laid on a lattice four times finer across the bedding than
    // along it, so each hole is a lens drawn out along the beds, and both
    // lattices are read through one ragged warp so their outlines tear.
    let voids = Voronoi::new().periods(32, 128).seed(349);
    let specks = Voronoi::new().periods(64, 256).seed(353);
    let g = MaterialGraph::builder("substances:cut-limestone")
        .param(Param::color("color", [0.53, 0.43, 0.29]))
        .param(Param::float("pitting", 1.0).range(0.0, 2.0))
        .param(Param::float("mineral_amount", 1.0).range(0.0, 1.0))
        .node("unit", GraphInput::float("unit_id", 0.5))
        .node("bed", Noise::perlin().periods(8, 64).octaves(3).seed(317))
        .node("grain", Noise::perlin().period(128).octaves(2).seed(319))
        .node("cloud", Noise::perlin().period(8).octaves(3).seed(323))
        .node(
            "grit",
            Blend::new(BlendMode::Dissolve, 0.0, 1.0).opacity(0.32),
        )
        .node(
            "colony_field",
            Noise::perlin().periods(16, 32).octaves(3).seed(331),
        )
        // Where colonies sit: a threshold that falls as `mineral_amount`
        // and the slab's own share rise.
        .node(
            "colony",
            Clamp::new(m(
                Div,
                m(
                    Sub,
                    "colony_field",
                    m(
                        Sub,
                        0.74,
                        m(Mul, p("mineral_amount"), remap("unit", 0.08, 0.2)),
                    ),
                ),
                0.08,
            )),
        )
        .node("rag_u", Noise::perlin().period(128).octaves(3).seed(359))
        .node("rag_v", Noise::perlin().period(128).octaves(3).seed(361))
        .node(
            "rag",
            Combine2::new(remap("rag_u", -1.0, 1.0), remap("rag_v", -1.0, 1.0)),
        )
        .node("void_cells", voids.output(VoronoiOutput::Cell))
        .node("void_field", voids)
        .node("void_id", Warp::new("void_cells", "rag").amount(0.003))
        .node(
            "void_distance",
            Warp::new("void_field", "rag").amount(0.003),
        )
        // Size is skewed: most voids are small, one in eight is large.
        .node("void_size", Math::unary(Fract, m(Mul, "void_id", 13.7)))
        .node(
            "void_radius",
            m(
                Add,
                0.1,
                m(
                    Mul,
                    m(Mul, "void_size", m(Mul, "void_size", "void_size")),
                    0.42,
                ),
            ),
        )
        .node(
            "void_profile",
            Clamp::new(m(
                Div,
                m(Sub, "void_radius", "void_distance"),
                "void_radius",
            )),
        )
        .node("void_present", m(Step, m(Mul, "colony", 0.55), "void_id"))
        // A void's floor is not uniformly black: some are shallow, silted
        // pockets, and the depth fades out from the middle.
        .node(
            "void_depth",
            remap(Math::unary(Fract, m(Mul, "void_id", 41.3)), 0.45, 1.0),
        )
        .node(
            "holes",
            m(
                Mul,
                m(Mul, Clamp::new(m(Mul, "void_profile", 2.2)), "void_depth"),
                "void_present",
            ),
        )
        .node("speck_cells", specks.output(VoronoiOutput::Cell))
        .node("speck_field", specks)
        .node("speck_id", Warp::new("speck_cells", "rag").amount(0.002))
        .node(
            "speck_profile",
            Clamp::new(m(
                Mul,
                m(
                    Sub,
                    remap(Math::unary(Fract, m(Mul, "speck_id", 7.9)), 0.08, 0.2),
                    Warp::new("speck_field", "rag").amount(0.002),
                ),
                7.0,
            )),
        )
        .node(
            "speck_holes",
            m(
                Mul,
                m(Mul, "speck_profile", 0.8),
                m(Step, m(Add, 0.02, m(Mul, "colony", 0.3)), "speck_id"),
            ),
        )
        .node("pores", m(Max, "holes", "speck_holes"))
        .node(
            "minerals",
            m(
                Mul,
                "colony",
                m(
                    Mul,
                    Levels::new("grain").in_range(0.35, 0.7),
                    p("mineral_amount"),
                ),
            ),
        )
        .node(
            "pits",
            m(
                Mul,
                p("pitting"),
                m(Add, m(Mul, "pores", 0.12), m(Mul, "minerals", 0.01)),
            ),
        )
        .node(
            "tone",
            m(
                Mul,
                m(Mul, remap("unit", 0.86, 1.1), remap("cloud", 0.93, 1.06)),
                m(Mul, remap("bed", 0.93, 1.05), remap("grain", 0.96, 1.03)),
            ),
        )
        .node(
            "stone_color",
            Mix::new(
                m(Mul, p("color"), "tone"),
                m(Mul, p("color"), Input::color([0.62, 0.64, 0.66])),
                m(Mul, "minerals", 0.9),
            ),
        )
        .node(
            "color",
            m(
                Mul,
                Mix::new(
                    "stone_color",
                    m(Mul, p("color"), Input::color([0.2, 0.18, 0.16])),
                    m(Mul, "pores", 0.9),
                ),
                remap("grit", 0.97, 1.025),
            ),
        )
        .node(
            "roughness",
            Clamp::new(m(
                Add,
                remap("grain", 0.67, 0.84),
                m(Add, m(Mul, "minerals", 0.1), m(Mul, "pores", 0.2)),
            )),
        )
        .node(
            "skin",
            m(
                Add,
                m(Add, remap("bed", 0.49, 0.52), remap("grain", -0.01, 0.01)),
                remap("grit", -0.001, 0.001),
            ),
        )
        .node("height", m(Sub, "skin", "pits"));
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0)
            .extra("minerals", "minerals")
            .extra("pores", "pores"),
        2.0,
        0.02,
    )
}

/// Warm honed limestone cladding in a broken bond, with fine light joints.
///
/// The `Tiles143` reference is travertine-like stone: near-square blocks of a
/// peach cream, a few colonies of dark solution voids per block, a honed face
/// that still catches a highlight, and each block set a hair off true.
#[must_use]
pub fn stone_cladding() -> MaterialGraph {
    laid_limestone(false)
}

/// Grey limestone blocks with deep joints and a bold chamfered margin.
///
/// The `Bricks066` reference reads as blocks proud of their joints: a flat,
/// tooled face, a chamfer a couple of centimetres wide falling to a joint a
/// centimetre deep, grime in the joints and rain streaks running down each
/// face from the block above. The margin is a band of the same layout's edge
/// distance, not another grid.
#[must_use]
pub fn ashlar_blocks() -> MaterialGraph {
    laid_limestone(true)
}

#[expect(
    clippy::too_many_lines,
    reason = "one composition with a dressed-margin variant"
)]
fn laid_limestone(ashlar: bool) -> MaterialGraph {
    use MathOp::{Add, Fract, Mul, Sub};
    let key = if ashlar {
        "library:ashlar-blocks"
    } else {
        "library:stone-cladding"
    };
    let color = if ashlar {
        [0.235, 0.238, 0.23]
    } else {
        [0.66, 0.45, 0.28]
    };
    // UV widths. The ashlar repeat is 2.4 m: a 12 mm joint and a 19 mm
    // chamfer; the cladding's is 2 m: a 1.6 mm joint and a 1 mm arris.
    let joint = if ashlar { 0.005 } else { 0.0008 };
    let bevel = if ashlar { 0.008 } else { 0.0005 };
    let (columns, rows, offset) = if ashlar { (4, 8, 0.5) } else { (5, 6, 0.34) };
    let grout_color = if ashlar {
        [0.05, 0.05, 0.045]
    } else {
        [0.42, 0.33, 0.22]
    };
    let g = MaterialGraph::builder(key)
        .param(Param::float("variation", 0.0).range(0.0, 1.0))
        .layer(
            "layout",
            Subgraph::new("layouts:slab-lattice")
                .param("columns", ParamValue::Int(columns))
                .param("rows", ParamValue::Int(rows))
                .param("offset", ParamValue::Float(offset))
                .param("joint_width", ParamValue::Float(joint))
                .param("bevel", ParamValue::Float(bevel))
                .param(
                    "irregularity",
                    ParamValue::Float(if ashlar { 0.8 } else { 0.2 }),
                ),
            &[
                SurfaceOutput::Extra("unit_id".into()),
                SurfaceOutput::Extra("mask".into()),
                SurfaceOutput::Extra("edge_distance".into()),
                SurfaceOutput::Extra("joint_depth".into()),
                SurfaceOutput::Extra("local_u".into()),
                SurfaceOutput::Extra("local_v".into()),
            ],
        )
        .node(
            "unit",
            Math::unary(Fract, m(Add, "layout.unit_id", p("variation"))),
        )
        .layer(
            "stone",
            Subgraph::new("substances:cut-limestone")
                .input("unit_id", "unit")
                .param("color", ParamValue::Color(color))
                .param(
                    "mineral_amount",
                    ParamValue::Float(if ashlar { 0.1 } else { 1.0 }),
                ),
            &CHANNELS,
        )
        .layer(
            "grout",
            Subgraph::new("substances:cast-cement")
                .param("color", ParamValue::Color(grout_color))
                .param("casting", ParamValue::Float(0.0)),
            &CHANNELS,
        )
        // Each block is set a little off true about both axes.
        .node(
            "tilt_u",
            m(Sub, Math::unary(Fract, m(Mul, "unit", 17.31)), 0.5),
        )
        .node(
            "tilt_v",
            m(Sub, Math::unary(Fract, m(Mul, "unit", 71.93)), 0.5),
        )
        .node(
            "set",
            m(
                Add,
                m(Mul, "tilt_u", m(Sub, "layout.local_u", 0.5)),
                m(Mul, "tilt_v", m(Sub, "layout.local_v", 0.5)),
            ),
        )
        // The ashlar's chamfer is the layout's own bevel, which this reads as
        // the margin; the cladding has none.
        .node(
            "margin",
            m(
                Mul,
                if ashlar { 1.0 } else { 0.0 },
                Invert::new(
                    Levels::new("layout.edge_distance")
                        .in_range(joint * 0.5 + bevel - 0.0015, joint * 0.5 + bevel),
                ),
            ),
        )
        .node(
            "streak",
            Noise::perlin().periods(128, 8).octaves(3).seed(953),
        )
        .node("weather", Noise::perlin().period(8).octaves(3).seed(957))
        // Rain runs off each block's top edge and down its face, fading as
        // it goes; the joints and the foot of each block hold dirt.
        .node(
            "runs",
            m(
                Mul,
                if ashlar { 1.0 } else { 0.0 },
                m(
                    Mul,
                    Levels::new("streak").in_range(0.45, 0.75),
                    m(
                        Mul,
                        Levels::new("layout.local_v")
                            .in_range(0.0, 1.0)
                            .out_range(0.9, 0.15),
                        remap("weather", 0.3, 1.0),
                    ),
                ),
            ),
        )
        .node(
            "edge_dirt",
            m(
                Mul,
                if ashlar { 0.3 } else { 0.12 },
                Invert::new(
                    Levels::new("layout.edge_distance")
                        .in_range(joint * 0.5, joint * 0.5 + bevel * 0.6),
                ),
            ),
        )
        .node(
            "face_height",
            m(
                Add,
                m(
                    Add,
                    if ashlar { 0.7 } else { 0.65 },
                    m(Mul, "set", if ashlar { 0.05 } else { 0.1 }),
                ),
                m(
                    Mul,
                    m(Sub, "stone.height", 0.50),
                    if ashlar { 1.4 } else { 1.0 },
                ),
            ),
        )
        .node(
            "face_color",
            m(
                Mul,
                m(
                    Mul,
                    "stone.base_color",
                    Invert::new(m(Add, m(Mul, "runs", 0.35), "edge_dirt")),
                ),
                if ashlar {
                    remap("unit", 0.78, 1.16)
                } else {
                    remap("unit", 0.95, 1.04)
                },
            ),
        )
        .node(
            "height",
            Mix::new(
                m(Mul, "grout.height", if ashlar { 0.3 } else { 0.7 }),
                "face_height",
                "layout.mask",
            ),
        )
        .node(
            "color",
            Mix::new("grout.base_color", "face_color", "layout.mask"),
        )
        .node(
            "roughness",
            Mix::new(
                0.93,
                if ashlar {
                    Input::from(Clamp::new(m(Add, "stone.roughness", m(Mul, "runs", 0.08))))
                } else {
                    Input::from(Clamp::new(m(Sub, "stone.roughness", 0.3)))
                },
                "layout.mask",
            ),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0)
            .extra("unit_id", "layout.unit_id")
            .extra("mask", "layout.mask")
            .extra("margin", "margin")
            .extra("joint_depth", "layout.joint_depth"),
        if ashlar { 2.4 } else { 2.0 },
        0.02,
    )
}
