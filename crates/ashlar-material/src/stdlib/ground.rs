//! Ground surfaces: bonded road aggregate and loose demolition fragments.
use super::surfaces::{CHANNELS, finish, m, p, rained_on, remap};
use crate::{
    MaterialGraph, MaterialGraphBuilder, MathOp, Param, ParamValue, PbrOutput, SurfaceOutput,
    nodes::{
        Adjust, Clamp, Colorize, Combine2, Invert, Levels, Math, Mix, Noise, Subgraph, Voronoi,
        VoronoiOutput, Warp,
    },
};

/// One population of angular aggregate: shrunken Voronoi polygons whose inset,
/// presence and tone are all read off the same cell, so a stone's outline,
/// crown and colour agree. Exports `{prefix}_mask` (the stone's footprint),
/// `{prefix}_crown` (a pyramid rising from the outline to the stone's centre,
/// planar per facet because the cell's edge distance is) and `{prefix}_id`.
fn aggregate(
    g: MaterialGraphBuilder,
    prefix: &str,
    period: u32,
    seed: u32,
    presence: f32,
    inset: (f32, f32),
) -> MaterialGraphBuilder {
    use MathOp::{Fract, Mul, Step, Sub};
    let edge = format!("{prefix}_edge");
    let id = format!("{prefix}_id");
    let size = format!("{prefix}_size");
    let over = format!("{prefix}_over");
    g.node(
        edge.clone(),
        Voronoi::new()
            .period(period)
            .seed(seed)
            .output(VoronoiOutput::Edge),
    )
    .node(
        id.clone(),
        Voronoi::new()
            .period(period)
            .seed(seed)
            .output(VoronoiOutput::Cell),
    )
    // A second hash of the cell, so size does not follow tone.
    .node(size.clone(), Math::unary(Fract, m(Mul, id.as_str(), 23.71)))
    .node(
        over.clone(),
        m(Sub, edge.as_str(), remap(size.as_str(), inset.0, inset.1)),
    )
    .node(
        format!("{prefix}_mask"),
        m(
            Mul,
            Levels::new(over.as_str()).in_range(0.0, 0.035),
            m(
                Step,
                presence,
                Math::unary(Fract, m(Mul, id.as_str(), 7.13)),
            ),
        ),
    )
    .node(
        format!("{prefix}_crown"),
        m(
            Mul,
            Levels::new(over.as_str()).in_range(0.0, 0.3).gamma(0.7),
            format!("{prefix}_mask"),
        ),
    )
}

/// Bitumen binder packed with fine angular road aggregate, in a 20 mm
/// height range.
///
/// Dense chippings on a 7.8 mm lattice and grit on a 3.9 mm one stand a
/// millimetre or two proud of a binder carrying sparse pores, with a few
/// larger pale chips on a 3 cm lattice. The ordinary stone reads mostly in
/// the relief and the roughness: traffic has polished the crowns smoother
/// than the binder and worn only a little of the coating off them, so its
/// albedo stays close to the binder's. Only the sparse pale chips are worn
/// clean. Exports `aggregate` (every stone's footprint) and `wear` (how much
/// stone the traffic has exposed).
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn bitumen_aggregate() -> MaterialGraph {
    use MathOp::{Add, Max, Mul, Sub};
    let g = MaterialGraph::builder("substances:bitumen-aggregate")
        .param(Param::color("binder", [0.064, 0.063, 0.061]))
        .param(Param::float("wear", 0.7).range(0.0, 1.0));
    let g = aggregate(g, "coarse", 256, 809, 0.85, (0.03, 0.2));
    let g = aggregate(g, "grit", 512, 811, 0.9, (0.03, 0.2));
    let g = aggregate(g, "pale", 64, 812, 0.05, (0.2, 0.3));
    let g = g
        .node("tone", Noise::perlin().period(8).octaves(4).seed(813))
        .node("coat", Noise::perlin().period(64).octaves(2).seed(815))
        .node("pore", Voronoi::new().period(256).seed(817))
        .node(
            "pore_id",
            Voronoi::new()
                .period(256)
                .seed(817)
                .output(VoronoiOutput::Cell),
        )
        .node(
            "pores",
            m(
                Mul,
                Levels::new("pore").in_range(0.32, 0.12),
                m(MathOp::Step, "pore_id", 0.9),
            ),
        )
        .node("fines", Noise::perlin().period(256).octaves(2).seed(819))
        .node(
            "aggregate",
            m(
                Max,
                "pale_mask",
                m(
                    Max,
                    "coarse_mask",
                    m(Mul, "grit_mask", Invert::new("coarse_mask")),
                ),
            ),
        )
        // Binder that fills the bed between the stones: slightly sunken, with
        // fine sand texture and sparse open pores.
        .node(
            "bed",
            m(
                Sub,
                m(Add, 0.46, remap("fines", -0.012, 0.012)),
                m(Mul, "pores", 0.07),
            ),
        )
        .node(
            "stone_height",
            m(
                Max,
                m(
                    Max,
                    m(Mul, "coarse_crown", remap("coarse_size", 0.08, 0.15)),
                    m(Mul, "grit_crown", 0.06),
                ),
                m(Mul, "pale_crown", 0.14),
            ),
        )
        .node("height", m(Add, "bed", "stone_height"))
        // How much of each stone stands clear of the binder: the polish.
        .node(
            "crown",
            Levels::new(m(Max, m(Add, "coarse_crown", "grit_crown"), "pale_crown"))
                .in_range(0.3, 0.9),
        )
        // Exposure: a little of the coating worn off ordinary crowns, and
        // the sparse pale chips worn clean.
        .node(
            "exposed",
            m(
                Mul,
                m(
                    Max,
                    m(
                        Mul,
                        m(Mul, "crown", Levels::new("coat").in_range(0.3, 0.7)),
                        0.45,
                    ),
                    m(
                        Mul,
                        Levels::new("pale_crown").in_range(0.1, 0.6),
                        remap("coat", 0.5, 0.9),
                    ),
                ),
                p("wear"),
            ),
        )
        .node("stone_id", Mix::new("grit_id", "coarse_id", "coarse_mask"))
        // Grey granite and limestone chippings a shade either side of the
        // binder, and a few warm flint-coloured ones.
        .node(
            "mineral",
            Colorize::new("stone_id").gradient([
                (0.0, [0.070, 0.070, 0.071]),
                (0.5, [0.105, 0.104, 0.101]),
                (0.88, [0.135, 0.132, 0.126]),
                (1.0, [0.150, 0.128, 0.100]),
            ]),
        )
        .node(
            "stone_color",
            Mix::new(
                "mineral",
                m(Mul, [0.165, 0.161, 0.152], remap("pale_id", 0.85, 1.1)),
                "pale_mask",
            ),
        )
        .node(
            "binder_color",
            m(
                Mul,
                p("binder"),
                m(Mul, remap("tone", 0.85, 1.15), remap("fines", 0.9, 1.1)),
            ),
        )
        .node(
            "coated_stone",
            Mix::new("binder_color", "stone_color", "exposed"),
        )
        .node(
            "color",
            m(
                Mul,
                Mix::new("binder_color", "coated_stone", "aggregate"),
                remap("pores", 1.0, 0.75),
            ),
        )
        // The binder is dull; polished crowns are the aggregate's main read.
        .node(
            "roughness",
            Clamp::new(m(
                Add,
                Mix::new(
                    remap("fines", 0.88, 0.95),
                    remap("stone_id", 0.64, 0.76),
                    m(Mul, "crown", p("wear")),
                ),
                m(Mul, "pores", 0.04),
            )),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0)
            .extra("aggregate", "aggregate")
            .extra("wear", "exposed"),
        2.0,
        0.02,
    )
}

/// Weathered road asphalt over a two metre repeat.
///
/// The `Road012B` reference is a mid-grey, oxidised wearing course: fine
/// aggregate everywhere, pale dusty streaks smeared along the traffic, a
/// darker patch of fresher repair, and sparse cracking — long meandering
/// lines that branch, narrow and die out, with wide uncracked stretches
/// between them and fine crazing only along the major cracks. `cracking`
/// gates every crack and only ever removes height; `oxidation` bleaches the
/// binder. `wet` is rain: the whole course darkened and smoothed, and puddles
/// standing in the sags (see `rained_on`); at zero it is the dry bake exactly.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn asphalt() -> MaterialGraph {
    use MathOp::{Add, Max, Mul, Sub};
    let g = MaterialGraph::builder("library:asphalt")
        .param(Param::float("cracking", 0.6).range(0.0, 1.0))
        .param(Param::float("oxidation", 0.65).range(0.0, 1.0))
        .param(Param::float("wet", 0.0).range(0.0, 1.0))
        .layer(
            "road",
            Subgraph::new("substances:bitumen-aggregate"),
            &CHANNELS,
        )
        .node(
            "road_aggregate",
            Subgraph::new("substances:bitumen-aggregate")
                .output(SurfaceOutput::Extra("aggregate".into())),
        )
        // Broad tone: sun and traffic bleach the binder unevenly.
        .node("weather", Noise::perlin().period(4).octaves(4).seed(821))
        .node("stain", Noise::perlin().period(16).octaves(3).seed(823))
        // A fresher repair: darker, finer and a millimetre proud.
        .node(
            "patch",
            Levels::new(Noise::perlin().period(2).octaves(3).seed(825)).in_range(0.66, 0.69),
        )
        // Two scales of wander: the long bends and the jag within them.
        .node(
            "drift_u",
            m(
                Add,
                remap(Noise::perlin().period(4).octaves(2).seed(829), -1.0, 1.0),
                remap(
                    Noise::perlin()
                        .period(32)
                        .octaves(3)
                        .persistence(0.6)
                        .seed(839),
                    -0.35,
                    0.35,
                ),
            ),
        )
        .node(
            "drift_v",
            m(
                Add,
                remap(Noise::perlin().period(4).octaves(2).seed(831), -1.0, 1.0),
                remap(
                    Noise::perlin()
                        .period(32)
                        .octaves(3)
                        .persistence(0.6)
                        .seed(841),
                    -0.35,
                    0.35,
                ),
            ),
        )
        .node("drift", Combine2::new("drift_u", "drift_v"))
        // Pale dust smeared along the traffic: streaks many times longer
        // than they are wide, bunched where a broad field lets them.
        .node(
            "streak",
            Noise::perlin()
                .periods(32, 2)
                .octaves(2)
                .persistence(0.45)
                .seed(827),
        )
        .node(
            "dust",
            m(
                Mul,
                m(
                    Mul,
                    Levels::new(Warp::new("streak", "drift").amount(0.015)).in_range(0.4, 0.66),
                    Levels::new(Noise::perlin().period(4).octaves(3).seed(837))
                        .in_range(0.46, 0.64),
                ),
                Invert::new("patch"),
            ),
        )
        // Major cracks: the edges of a half-metre cell network, pushed several
        // centimetres off their lines. Most edges are never drawn: a
        // low-frequency field says where the road is cracking at all, a
        // finer one breaks each run into lengths, and both set the crack's
        // *width* rather than gate it, so a line narrows and dies out instead
        // of stopping at a cell corner.
        .node(
            "major",
            Warp::new(
                Voronoi::new()
                    .period(4)
                    .seed(833)
                    .output(VoronoiOutput::Edge),
                "drift",
            )
            .amount(0.03),
        )
        .node(
            "region",
            Levels::new(Noise::perlin().period(4).octaves(3).seed(835)).in_range(0.42, 0.56),
        )
        .node(
            "runs",
            Levels::new(Noise::perlin().period(16).octaves(2).seed(843)).in_range(0.34, 0.5),
        )
        .node(
            "major_open",
            m(Mul, m(Mul, "region", "runs"), p("cracking")),
        )
        // Width in cell units of a half-metre cell: up to 3 mm where the run
        // is fully open, pinching to nothing at its ends.
        .node(
            "major_width",
            m(
                Sub,
                m(
                    Mul,
                    "major_open",
                    remap(Noise::perlin().period(32).seed(845), 0.003, 0.007),
                ),
                0.0008,
            ),
        )
        .node(
            "major_line",
            Levels::new(m(Sub, "major", "major_width")).in_range(0.0012, -0.0012),
        )
        // Branches: a finer network, sparser still and only where the road
        // is already cracking, so it reads as lines forking off the majors.
        .node(
            "branch",
            Warp::new(
                Voronoi::new()
                    .period(8)
                    .seed(847)
                    .output(VoronoiOutput::Edge),
                "drift",
            )
            .amount(0.02),
        )
        .node(
            "branch_open",
            m(
                Mul,
                m(
                    Mul,
                    "region",
                    Levels::new(Noise::perlin().period(16).octaves(2).seed(849))
                        .in_range(0.5, 0.62),
                ),
                p("cracking"),
            ),
        )
        .node(
            "branch_line",
            Levels::new(m(
                Sub,
                "branch",
                m(Sub, m(Mul, "branch_open", 0.008), 0.0016),
            ))
            .in_range(0.0024, -0.0024),
        )
        .node(
            "shoulder",
            m(
                Mul,
                Levels::new("major").in_range(0.03, 0.0).gamma(2.0),
                "major_open",
            ),
        )
        // Crazing only in a band a few centimetres either side of a major.
        .node(
            "craze_bias",
            m(
                Mul,
                Levels::new("major").in_range(0.06, 0.015),
                m(Mul, "major_open", Invert::new("patch")),
            ),
        )
        .node(
            "crazing",
            Subgraph::new("patterns:cracks")
                .input("mask", "craze_bias")
                .param("width", ParamValue::Float(0.0004))
                .param("warp", ParamValue::Float(0.012))
                .output(SurfaceOutput::Extra("crack_mask".into())),
        )
        .node(
            "cracks",
            m(
                Max,
                m(Max, "major_line", m(Mul, "branch_line", 0.85)),
                m(Mul, "crazing", 0.7),
            ),
        )
        .node(
            "binder_tone",
            m(
                Add,
                remap("weather", 0.8, 1.25),
                m(Mul, remap("stain", -0.1, 0.2), p("oxidation")),
            ),
        )
        .node(
            "surface",
            m(
                Mul,
                "road.base_color",
                Mix::new("binder_tone", 0.72, "patch"),
            ),
        )
        .node(
            "dusted",
            Mix::new(
                "surface",
                m(Mul, [0.19, 0.18, 0.165], remap("stain", 0.85, 1.1)),
                m(
                    Mul,
                    "dust",
                    m(Mul, remap("road_aggregate", 0.6, 0.35), p("oxidation")),
                ),
            ),
        )
        .node(
            "dry_color",
            m(
                Mul,
                "dusted",
                m(Mul, remap("cracks", 1.0, 0.2), remap("shoulder", 1.0, 0.82)),
            ),
        )
        .node(
            "dry_height",
            m(
                Sub,
                m(Add, "road.height", m(Mul, "patch", 0.03)),
                m(Add, m(Mul, "cracks", 0.12), m(Mul, "shoulder", 0.03)),
            ),
        )
        .node(
            "dry_roughness",
            Clamp::new(Mix::new(
                m(Add, "road.roughness", m(Mul, "dust", 0.04)),
                0.95,
                "cracks",
            )),
        )
        // Where rain stands: one broad, ragged sag in the repeat, so the
        // puddles of a wide junction do not stand in a grid, and the relief
        // itself, so a puddle's shore follows the aggregate.
        .node(
            "pond",
            m(
                Add,
                m(Mul, Noise::perlin().period(1).octaves(4).seed(851), 0.7),
                m(Mul, "dry_height", 0.5),
            ),
        );
    let g = rained_on(
        g,
        ["dry_color", "dry_roughness", "dry_height"],
        "pond",
        0.57,
        0.6,
    );
    finish(
        g,
        PbrOutput::new()
            .base_color("rain.base_color")
            .roughness("rain.roughness")
            .metallic(0.0)
            .extra("cracks", "cracks"),
        2.0,
        0.02,
    )
}

/// Demolition rubble over a trodden dust bed, in a 2.1 m repeat and an 80 mm
/// height range.
///
/// Four fragment populations — 13, 6.6, 3.3 and 1.6 cm lattices — are each
/// drawn as shrunken Voronoi polygons roofed by the cell's own edge distance,
/// which is planar per facet, and capped at a per-fragment height, so every
/// piece is an angular broken lump with flat faces rather than a blob. Each
/// piece owns one hash that decides whether it is concrete, stone or brick,
/// its tone, how proud it stands and how deeply it is buried. A dust bed
/// rises and falls across the repeat and buries the pieces it overtakes,
/// which is what clusters them into heaps with bare trodden ground between,
/// and dust settles on every piece where it meets the bed. Exports `pieces`
/// (the exposed fragments) and `dust`.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "four fragment populations share the same construction"
)]
pub fn rubble() -> MaterialGraph {
    use MathOp::{Add, Fract, Max, Min, Mul, Step, Sub};
    let mut g = MaterialGraph::builder("library:rubble")
        .param(Param::float("dust", 0.6).range(0.0, 1.0))
        .layer("cement", Subgraph::new("substances:cast-cement"), &CHANNELS)
        .layer("clay", Subgraph::new("substances:fired-clay"), &CHANNELS)
        .node(
            "heaps",
            Noise::perlin()
                .period(4)
                .octaves(4)
                .persistence(0.55)
                .seed(823),
        )
        .node("fines", Noise::perlin().period(256).octaves(2).seed(825))
        .node(
            "face_grain",
            Noise::perlin().period(128).octaves(2).seed(827),
        )
        .node("mottle", Noise::perlin().period(32).octaves(3).seed(853))
        .node(
            "grit_edge",
            Voronoi::new()
                .period(512)
                .seed(829)
                .output(VoronoiOutput::Edge),
        )
        .node(
            "grit_id",
            Voronoi::new()
                .period(512)
                .seed(829)
                .output(VoronoiOutput::Cell),
        )
        .node(
            "grit",
            m(
                Mul,
                Levels::new("grit_edge").in_range(0.08, 0.2),
                m(Step, 0.5, "grit_id"),
            ),
        )
        // The bed: trodden dust, low in the hollows and heaped where the
        // debris was dumped, with sand grit through it.
        .node(
            "bed",
            m(
                Add,
                m(
                    Add,
                    0.12,
                    Levels::new("heaps")
                        .in_range(0.3, 0.75)
                        .gamma(1.3)
                        .out_range(0.0, 0.26),
                ),
                m(
                    Add,
                    remap("fines", -0.012, 0.012),
                    m(Mul, "grit", m(Mul, "grit_id", 0.03)),
                ),
            ),
        )
        // Where the heap stands, pieces sit higher in it as well as being
        // more numerous: the lift is added to every piece below.
        .node(
            "lift",
            Levels::new("heaps")
                .in_range(0.22, 0.55)
                .out_range(-0.2, 0.24),
        )
        .node(
            "stone_tint",
            Colorize::new(Noise::perlin().period(2).octaves(2).seed(831)).gradient([
                (0.0, [0.060, 0.058, 0.064]),
                (0.5, [0.125, 0.118, 0.126]),
                (1.0, [0.170, 0.158, 0.145]),
            ]),
        );
    // (prefix, period, seed, presence, inset, rise, cap range)
    let populations: [(&str, u32, u32, f32, (f32, f32), f32, (f32, f32)); 4] = [
        ("boulder", 16, 833, 0.40, (0.05, 0.2), 0.60, (0.55, 1.0)),
        ("cobble", 32, 839, 0.65, (0.04, 0.18), 0.42, (0.5, 1.0)),
        ("gravel", 64, 843, 0.85, (0.03, 0.16), 0.28, (0.45, 1.0)),
        ("chip", 128, 847, 0.9, (0.03, 0.14), 0.15, (0.5, 1.0)),
    ];
    for (prefix, period, seed, presence, inset, rise, cap) in populations {
        let n = |s: &str| format!("{prefix}_{s}");
        g = g
            .node(
                n("edge"),
                Voronoi::new()
                    .period(period)
                    .seed(seed)
                    .output(VoronoiOutput::Edge),
            )
            .node(
                n("id"),
                Voronoi::new()
                    .period(period)
                    .seed(seed)
                    .output(VoronoiOutput::Cell),
            )
            // Independent hashes of the one cell: size, cap, burial, kind.
            .node(n("size"), Math::unary(Fract, m(Mul, n("id"), 23.71)))
            .node(n("cap"), Math::unary(Fract, m(Mul, n("id"), 41.3)))
            .node(n("sink"), Math::unary(Fract, m(Mul, n("id"), 57.9)))
            .node(n("kind"), Math::unary(Fract, m(Mul, n("id"), 13.37)))
            .node(
                n("present"),
                m(Step, presence, Math::unary(Fract, m(Mul, n("id"), 7.13))),
            )
            .node(
                n("over"),
                m(Sub, n("edge"), remap(n("size"), inset.0, inset.1)),
            )
            .node(
                n("mask"),
                m(
                    Mul,
                    Levels::new(n("over")).in_range(0.0, 0.012),
                    n("present"),
                ),
            )
            // The hipped roof of the polygon, cut flat at the piece's own cap.
            .node(
                n("roof"),
                m(
                    Min,
                    Levels::new(n("over")).in_range(0.0, 0.22),
                    remap(n("cap"), cap.0, cap.1),
                ),
            )
            .node(
                n("height"),
                m(
                    Mul,
                    n("mask"),
                    m(
                        Add,
                        m(Add, "lift", remap(n("sink"), -0.12, 0.08)),
                        m(
                            Mul,
                            m(Add, n("roof"), remap("face_grain", -0.03, 0.03)),
                            rise,
                        ),
                    ),
                ),
            )
            .node(
                n("tone"),
                m(
                    Mul,
                    m(
                        Mul,
                        m(
                            Mul,
                            remap(n("id"), 0.6, 1.2),
                            remap("face_grain", 0.8, 1.12),
                        ),
                        remap("fines", 0.84, 1.14),
                    ),
                    m(
                        Mul,
                        remap("mottle", 0.82, 1.1),
                        Levels::new(n("roof"))
                            .in_range(0.0, 0.6)
                            .out_range(0.6, 1.1),
                    ),
                ),
            )
            // Mostly broken concrete, then grey stone, then a little brick.
            .node(
                n("color"),
                m(
                    Mul,
                    Mix::new(
                        Mix::new(
                            m(Mul, "cement.base_color", 1.05),
                            "stone_tint",
                            m(Step, n("kind"), 0.5),
                        ),
                        Adjust::new(m(Mul, "clay.base_color", 0.7)).saturation(0.7),
                        m(Step, n("kind"), 0.92),
                    ),
                    n("tone"),
                ),
            )
            .node(n("rough"), Mix::new(0.9, 0.74, m(Step, n("kind"), 0.55)));
    }
    // Stack the populations: whichever stands highest owns the texel.
    let mut top = "boulder_height".to_string();
    let mut color = "boulder_color".to_string();
    let mut rough = "boulder_rough".to_string();
    for prefix in ["cobble", "gravel", "chip"] {
        let h = format!("{prefix}_height");
        let take = format!("{prefix}_take");
        g = g
            .node(take.clone(), m(Step, h.as_str(), top.as_str()))
            .node(format!("{prefix}_top"), m(Max, top.as_str(), h.as_str()))
            .node(
                format!("{prefix}_stack_color"),
                Mix::new(color.as_str(), format!("{prefix}_color"), take.as_str()),
            )
            .node(
                format!("{prefix}_stack_rough"),
                Mix::new(rough.as_str(), format!("{prefix}_rough"), take.as_str()),
            );
        top = format!("{prefix}_top");
        color = format!("{prefix}_stack_color");
        rough = format!("{prefix}_stack_rough");
    }
    g = g
        .node(
            "pieces",
            Levels::new(m(Sub, top.as_str(), "bed")).in_range(0.0, 0.01),
        )
        .node("height", Clamp::new(m(Max, top.as_str(), "bed")))
        // Dust clings where a piece meets the bed and films the whole heap
        // unevenly; the upper faces of proud pieces stay cleanest.
        .node(
            "dust",
            m(
                Mul,
                p("dust"),
                m(
                    Max,
                    Levels::new(m(Sub, top.as_str(), "bed")).in_range(0.07, 0.0),
                    Levels::new(Noise::perlin().period(8).octaves(3).seed(851)).in_range(0.45, 0.8),
                ),
            ),
        )
        .node(
            "bed_color",
            m(
                Mul,
                Mix::new(
                    [0.215, 0.19, 0.155],
                    [0.085, 0.078, 0.068],
                    Levels::new("heaps").in_range(0.35, 0.55),
                ),
                m(
                    Mul,
                    remap("fines", 0.85, 1.12),
                    Mix::new(1.0, remap("grit_id", 0.45, 1.3), "grit"),
                ),
            ),
        )
        .node(
            "piece_color",
            Mix::new(
                color.as_str(),
                m(Mul, "bed_color", 1.08),
                m(Mul, "dust", 0.75),
            ),
        )
        .node("color", Mix::new("bed_color", "piece_color", "pieces"))
        .node(
            "roughness",
            Mix::new(
                remap("fines", 0.9, 0.97),
                rough.as_str(),
                m(Mul, "pieces", Invert::new(m(Mul, "dust", 0.7))),
            ),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0)
            .extra("pieces", "pieces")
            .extra("dust", "dust"),
        2.1,
        0.08,
    )
}
