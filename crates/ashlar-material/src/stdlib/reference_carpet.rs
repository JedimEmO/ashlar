//! A short moss carpet: the strand vocabulary with no grass in it.
//!
//! `library:grass` is one material, and one material does not show that a
//! vocabulary is general. This is the second: the same [`StrandLayer`], the
//! same [`StrandRelief`] and the same level-of-detail chain over a surface that
//! is not a lawn and does not behave like one.
//!
//! Four things here are the opposite of what the grass does, and each is a
//! control the vocabulary already had:
//!
//! - [`StrandProfile::Fibre`] rather than `Blade`. A moss shoot is a round
//!   thing seen from every side, so it is a three-sided tube; a grass blade is
//!   a ribbon. It costs three times the triangles per segment, which is why
//!   this layer is one segment and half the count.
//! - **Upright.** `lean` runs from 0.05 to 0.28 where the grass runs from 0.34
//!   to 0.62. Moss stands; grass lies over. The cost of standing up is that the
//!   relief of a strand seen from directly above is a *dot* rather than a
//!   stroke — the splat's own limitation, and here it is the right answer,
//!   because a moss cushion seen from above is a field of dots.
//! - **No flow.** The grass combs along the contours of its clump field. Moss
//!   has no grain at all, so the direction is a turn read straight off a noise
//!   and then varied by a full half turn either way.
//! - **A mask rather than a field.** The density is
//!   [`stdlib::moss`](crate::stdlib::moss)'s own coverage mask, which
//!   is a shared compound the two moss-over-brick variants already wear. The
//!   shoots grow exactly where the moss is and nowhere else, which is what
//!   makes this a *coat* on a surface rather than a surface of its own.
//!
//! The pile is [`HEIGHT_SCALE_METRES`] tall — half the grass — and a full
//! detail repeat is about 470 000 triangles.
#![allow(clippy::too_many_lines, reason = "authored reference recipe")]
use super::reference_support::{TILE_METRES, m, p, remap};
use crate::{
    Input, MaterialGraph,
    MathOp::{Add, Mul},
    Param, ParamValue, PbrOutput, StrandLayer, StrandProfile,
    nodes::{
        Clamp, Colorize, Direction, Levels, Mix, Noise, OcclusionFromHeight, StrandRelief,
        StrandReliefOutput, Subgraph, SurfaceOutput,
    },
};

/// Physical height range of the cushion.
pub const HEIGHT_SCALE_METRES: f32 = 0.009;

/// Shoots across the two-metre repeat, on each axis.
///
/// The grass lattice, so the two materials share a period and a reader
/// comparing them is comparing the strands rather than the sampling.
pub const SHOOT_COUNT: u32 = 256;

/// Shoots standing in each cell.
///
/// Two, where the grass fibres stand three. A [`StrandProfile::Fibre`] is six
/// triangles a segment against a ribbon's two, so the same triangle budget buys
/// a third of the shoots — which is the honest trade the profile makes and the
/// reason `Blade` is still the default.
pub const SHOOTS_PER_CELL: u32 = 2;

/// How long a shoot is, in metres. The whole cushion, because a moss shoot is
/// the cushion rather than something standing on it.
pub const SHOOT_LENGTH_METRES: f32 = HEIGHT_SCALE_METRES;

/// How wide a shoot is, in metres.
///
/// 1.2 mm, a little over one texel of the shipped 2048² maps. Under one texel
/// the splat can no longer draw the shoot and the bake warns by path, so this
/// is the floor rather than a choice about how moss looks.
pub const SHOOT_WIDTH_METRES: f32 = 0.0012;

/// A moss cushion over damp ground, as a two-metre repeat.
pub fn moss_carpet(seed: u32) -> MaterialGraph {
    let g = MaterialGraph::builder("library:moss-carpet")
        .param(Param::float("lushness", 0.82).range(0.0, 1.0))
        .param(Param::float("tip_brightness", 0.55).range(0.0, 1.0))
        // The ground the cushion sits on. It is barely seen — that is what a
        // carpet is — so it is two noises and a ramp rather than a substrate
        // graph of its own.
        .node(
            "patches",
            Noise::perlin()
                .period(6)
                .octaves(4)
                .persistence(0.58)
                .seed(seed.wrapping_add(13)),
        )
        .node(
            "grain",
            Noise::perlin()
                .period(96)
                .octaves(3)
                .persistence(0.55)
                .seed(seed.wrapping_add(31)),
        )
        .node(
            "soil_tone",
            Clamp::new(m(
                Add,
                0.08,
                m(Add, m(Mul, "patches", 0.56), m(Mul, "grain", 0.30)),
            )),
        )
        .node(
            "soil_color",
            Colorize::new("soil_tone").gradient([
                (0.0, [0.014, 0.011, 0.008]),
                (0.55, [0.048, 0.038, 0.024]),
                (1.0, [0.110, 0.092, 0.060]),
            ]),
        )
        .node("soil_height", Levels::new("patches").out_range(0.18, 0.52))
        // Where the moss takes: the hollows, which is what `shelter` means to
        // the shared compound and what it means everywhere else it is used.
        .node("shelter", remap("patches", 0.18, 1.0))
        .layer(
            "moss",
            Subgraph::new("weathering:moss")
                .input("base_color", "soil_color")
                .input("roughness", Input::float(0.93))
                .input("metallic", Input::float(0.0))
                .input("height", "soil_height")
                .input("shelter", "shelter")
                .input("coverage", p("lushness"))
                .param("amount", ParamValue::Float(1.0))
                .param("depth", ParamValue::Float(0.05)),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Metallic,
                SurfaceOutput::Height,
                SurfaceOutput::Extra("mask".into()),
            ],
        );
    // `library::finish` is not used here, and the reason is the subgraph:
    // it ends in `build()`, which resolves instances against no library at all
    // and refuses `weathering:moss` by path. The two moss-over-brick variants
    // hand back an unbuilt graph for the same reason, and the library is what
    // builds them.
    let relief = HEIGHT_SCALE_METRES / TILE_METRES;
    surface(shoots(g, seed))
        .node("physical_height", m(Mul, "height", relief))
        .node(
            "ao",
            OcclusionFromHeight::new("physical_height").radius(0.015),
        )
        .output(
            PbrOutput::new()
                .base_color("color")
                .roughness("rough")
                .metallic("moss.metallic")
                .occlusion("ao")
                .height("height")
                .normal_strength(relief)
                .extra("moss_mask", "moss.mask")
                .extra("shelter", "shelter")
                .extra("shoot_cover", "shoot_cover")
                .extra("pile", "pile"),
        )
        // `relief` above is metres per repeat and the shoots are metres, so the
        // repeat those two were authored against belongs in the graph.
        .tile_metres([TILE_METRES; 2])
        .into_graph()
}

/// The `shoots` layer, over the shared moss mask.
fn shoots(g: crate::MaterialGraphBuilder, seed: u32) -> crate::MaterialGraphBuilder {
    g
        // A turn read straight off a noise, with no quarter turn on it: moss
        // has no grain, so there is no contour to comb along. It is here at all
        // because a direction of zero is an upright strand with no plane for
        // the lean and the bend to tilt into, and a cushion of perfectly
        // vertical needles is a bed of nails.
        .node("shoot_flow", Direction::from_angle("grain"))
        // The shoots stand where the moss is. Below a third of the mask there
        // is bare ground, which is what keeps the edge of a patch an edge.
        .node("shoot_cover_field", remap("moss.mask", 0.30, 0.96))
        .node(
            "shoot_length",
            remap(
                m(Add, m(Mul, "moss.mask", 0.6), m(Mul, "grain", 0.4)),
                0.5,
                1.0,
            ),
        )
        // Upright, and this is the whole difference from the grass: 5 to 28
        // hundredths of a quarter turn, against the lawn's 34 to 62.
        .node("shoot_lean", remap("grain", 0.05, 0.28))
        .node("shoot_bend", remap("patches", 0.10, 0.32))
        .node("shoot_root_color", m(Mul, "moss.base_color", 0.70))
        .node(
            "shoot_tip_color",
            Mix::new(
                "moss.base_color",
                [0.185, 0.290, 0.042],
                p("tip_brightness"),
            ),
        )
        .strands(
            "shoots",
            StrandLayer::new()
                .count(SHOOT_COUNT)
                .seed(seed.wrapping_add(211))
                .profile(StrandProfile::Fibre)
                .density("shoot_cover_field")
                .length("shoot_length")
                .lean("shoot_lean")
                .bend("shoot_bend")
                .direction("shoot_flow")
                .colors("shoot_root_color", "shoot_tip_color")
                .roughness(Input::float(0.93))
                .metres(SHOOT_LENGTH_METRES, SHOOT_WIDTH_METRES)
                .per_cell(SHOOTS_PER_CELL)
                // A tighter clump lattice than the lawn's and a weaker pull: a
                // moss cushion is many small heads rather than a few tufts, and
                // the heads do not lean into each other the way blades do.
                .clumps(96, 0.40, 0.18)
                .clump_tint(0.28)
                .midpoint(0.20)
                .facing_variation(1.0)
                .height_offset(SHOOT_LENGTH_METRES * 0.42)
                .length_variation(0.52)
                .width_variation(0.35)
                // A full half turn either way, which with no flow field under
                // it is a shoot pointing anywhere at all.
                .direction_variation(1.0)
                .lean_variation(0.60)
                .bend_variation(0.50)
                .segments(1)
                .taper(0.78)
                .root_occlusion(0.25),
        )
}

/// The surface: the same shoots, splatted from above.
fn surface(g: crate::MaterialGraphBuilder) -> crate::MaterialGraphBuilder {
    let shoot = |output| StrandRelief::new("shoots", TILE_METRES).output(output);
    g.node("shoot_cover", shoot(StrandReliefOutput::Coverage))
        .node("shoot_mass", shoot(StrandReliefOutput::Mass))
        .node("shoot_color", shoot(StrandReliefOutput::Color))
        .node("shoot_height", shoot(StrandReliefOutput::Height))
        .node(
            "pile",
            Levels::new("shoot_mass").in_range(0.0, 4.0).gamma(0.70),
        )
        // An upright layer stands nearly its whole length over the texel it
        // covers, so the range tops out much higher than the lawn's.
        .node("crown", Levels::new("shoot_height").in_range(0.0, 0.70))
        // The same arrangement the grass ended on, and for the same reason: the
        // ground under a cushion is the darkest thing in the picture, and what
        // is lit is whatever is standing highest over the texel.
        .node("under_color", m(Mul, "moss.base_color", 0.34))
        .node(
            "color",
            m(
                Mul,
                Mix::new("under_color", "shoot_color", "shoot_cover"),
                remap("crown", 0.34, 1.0),
            ),
        )
        .node(
            "height",
            Clamp::new(m(
                Add,
                0.08,
                m(
                    Add,
                    m(Mul, "moss.height", 0.18),
                    m(
                        Add,
                        m(Mul, "pile", 0.48),
                        m(Mul, m(Mul, "crown", "shoot_cover"), 0.22),
                    ),
                ),
            )),
        )
        .node("rough", Clamp::new("moss.roughness").range(0.70, 1.0))
}
