//! Stone-and-grout reconstruction of SOI's preserved Designer graph.
//! See docs/material-references/soi-cobblestone for source IDs and known differences.
//! Noise generators and filter units are Ashlar's, so this is not pixel parity.
#![allow(clippy::too_many_lines, reason = "authored reference recipe")]
use super::reference_support::{finish, m, p, rectangles, remap};
use crate::{
    MaterialGraph,
    MathOp::{Abs, Add, Cos, Div, Fract, Max, Min, Mul, Sin, Sqrt, Step, Sub},
    Param, PbrOutput,
    nodes::{
        Blend, BlendMode, Blur, BrickOutput, Bricks, Clamp, Colorize, Curvature, Curve, Distance,
        GradientWarp, IntensityWarp, Invert, Levels, Math, Mix, Noise, Scratches, SlopeMode,
        Voronoi, VoronoiOutput,
    },
};

/// Half the reach of the signed stone profile, in UV. At 2048 this is 25 px
/// each way, which is where the reference height reaches its plateau inside a
/// stone and where its skirt has met the grout outside one.
const PROFILE_REACH: f32 = 0.012;

/// Width of the stone-to-grout join, in height. The two surfaces meet over this
/// band instead of creasing, which is what the source's Height Blend does.
/// Narrow: at 0.045 the band moved the profile's steepest point six pixels out
/// into the joint, where the reference's steepest point is the crossing itself.
const JOIN_WIDTH: f32 = 0.032;

/// SOI's first preset supplies the 8 × 10 running bond and slate palette.
/// Controls are Ashlar units, not a literal import of Designer intensities.
pub fn cobblestone(seed: u32) -> MaterialGraph {
    let g = rectangles(
        MaterialGraph::builder("library:soi-cobblestone"),
        8.0,
        10.0,
        0.5,
    )
    .param(Param::float("warping", 1.0).range(0.0, 2.0))
    .param(Param::float("edge_damage", 1.0).range(0.0, 2.0))
    .param(Param::float("sloping", 0.4).range(0.0, 0.7))
    .param(Param::float("surface_noise", 1.0).range(0.0, 2.0))
    // The bed is low. Read against the reference's own height the joint floor
    // sits at 0.10 to 0.20 of the range with the pebbles standing on top of it,
    // and every stone — the lowest ones included — clears it by a tenth, which
    // is what draws the continuous dark line all the way round each silhouette.
    // Chasing a joint median measured through a mask that had already swallowed
    // the chamfer put this at 0.352, and at that level the low stones drowned:
    // the dark net went soft and the joints read as lumpy mid-grey channels.
    .param(Param::float("grout_height", 0.180).range(0.1, 0.75))
    .param(Param::float("roughness", 0.645).range(0.35, 0.85))
    // The one knob for both the rim darkening and the joint warmth:
    // it scales the mask builder that the rust colour, the roughness
    // lift and the grout bleed all read.
    .param(Param::float("dirt", 1.0).range(0.0, 2.0))
    // 1294217152 / 1294217160: a smoothed cellular displacement guide.
    .node(
        "cells",
        Voronoi::new().period(16).seed(seed.wrapping_add(1)),
    )
    .node("guide", Blur::new("cells").radius(0.012))
    .node("warp_guide", m(Mul, "guide", p("warping")))
    .node(
        "identity",
        Bricks::new()
            .columns(8)
            .rows(10)
            .seed(seed)
            .output(BrickOutput::Id),
    )
    .node(
        "turn",
        Bricks::new()
            .columns(8)
            .rows(10)
            .seed(seed.wrapping_add(2))
            .output(BrickOutput::Id),
    )
    .node(
        "stone_id",
        GradientWarp::new("identity", "warp_guide").amount(0.00035),
    )
    // The reference's stones are not rectangles with a softened threshold: their
    // corners are properly knocked off, fifteen to forty pixels of radius and a
    // different amount at each of the four. Rounding by blurring the mask cannot
    // say that — one radius for the whole wall, and it eats the straight runs
    // along with the corners. A rounded-rectangle distance says it directly:
    // inside a corner the field is the distance to a circle of that radius,
    // everywhere else it is the cell distance unchanged, so the radius is an
    // authored number and the sides stay where they were. The noise is coarse
    // enough that each corner of a stone reads its own value.
    .node(
        "corner_noise",
        Noise::perlin()
            .period(16)
            .octaves(2)
            .seed(seed.wrapping_add(13)),
    )
    .node("corner_stone", Math::unary(Fract, m(Mul, "stone_id", 23.0)))
    .node(
        "corner_r",
        m(
            Mul,
            remap("corner_noise", 0.60, 1.30),
            remap("corner_stone", 0.0068, 0.0118),
        ),
    )
    .node("corner_dx", m(Max, m(Sub, "corner_r", "dx"), 0.0))
    .node("corner_dy", m(Max, m(Sub, "corner_r", "dy"), 0.0))
    .node(
        "rounded_plan",
        m(
            Add,
            m(
                Sub,
                "corner_r",
                Math::unary(
                    Sqrt,
                    m(
                        Add,
                        m(Mul, "corner_dx", "corner_dx"),
                        m(Mul, "corner_dy", "corner_dy"),
                    ),
                ),
            ),
            // Past the corner quadrant this term restores the cell distance
            // exactly, so nothing caps the field and a wide inset still has
            // somewhere to cut.
            m(Max, m(Sub, "edge_distance", "corner_r"), 0.0),
        ),
    )
    // Equivalent boundary source to the Tile Sampler → Edge Detect pair.
    // Measure a real warped mask, so bevel width follows the distorted edges.
    .node(
        "boundary",
        GradientWarp::new("rounded_plan", "warp_guide").amount(0.00035),
    )
    // Source Tile Sampler 1294158122 has no size randomness, but the reference
    // joints still vary from 8 to 25 px half-width. A second per-stone number
    // decorrelated out of the warped identity insets each stone's own four
    // sides by its own amount, so a joint is asymmetric: one neighbour gives
    // more ground than the other, which is what a size random would have done.
    // Three to nine pixels a side. The damage below erodes another four or
    // five, so the gap the height shows comes out at the reference's own eight
    // to thirty-five with a median near twenty; measured, the wider inset that
    // stood here gave a median of thirty.
    .node("stone_inset", Math::unary(Fract, m(Mul, "stone_id", 7.0)))
    // The reference's sides are not straight and they are not ragged either:
    // each one bows and leans over its whole length, so a stone reads as an
    // individually distorted quad. A 128-pixel field at ±2 px is that bow.
    // A third of what it was: at ±6 px two facing bows could add to a
    // twelve-pixel swing in one joint's width, which reads as a joint that
    // swells and pinches rather than one that wanders.
    .node(
        "edge_wander",
        Noise::perlin()
            .period(16)
            .octaves(2)
            .seed(seed.wrapping_add(3)),
    )
    // Sparse blobs, not a periodic ripple: a thresholded mid-scale noise bites
    // a few chips out of each silhouette and leaves the rest of it straight.
    // Smaller and rarer than it was. At a 0.56 gate and eleven pixels deep a
    // bite landed on most sides of most stones and took the joint out to sixty
    // pixels where it fell; the reference has a knocked corner eight to fifteen
    // pixels deep every few stones and an otherwise even net between them.
    .node(
        "edge_chips",
        Levels::new(
            Noise::perlin()
                .period(32)
                .octaves(2)
                .seed(seed.wrapping_add(17)),
        )
        .in_range(0.64, 0.90),
    )
    // Two and a half pixels at a sixteen-pixel scale. Measured as the ratio of
    // a silhouette's perimeter to a smooth shape of the same area, the
    // reference's outlines are a quarter longer than a plain rectangle's: the
    // corners are knocked off, but the runs between them are not drawn with a
    // ruler either. This is the small part of that; the bow above is the large
    // part. Any more and it is a comb again.
    .node(
        "edge_fray",
        Noise::perlin()
            .period(64)
            .octaves(2)
            .seed(seed.wrapping_add(53)),
    )
    .node(
        "edge_width",
        m(
            Add,
            m(Add, 0.0016, m(Mul, "stone_inset", 0.0030)),
            m(
                Add,
                remap("edge_wander", -0.0011, 0.0011),
                m(
                    Add,
                    remap("edge_fray", -0.0009, 0.0009),
                    m(Mul, "edge_chips", 0.0040),
                ),
            ),
        ),
    )
    .node(
        "interior",
        Levels::new(m(Sub, "boundary", "edge_width")).in_range(0.0, 0.0007),
    )
    // The damage walks this soft field rather than the thresholded mask: a Min
    // slope blur over a step leaves radial streaks, while over a ramp it moves
    // the crossing and the bite keeps a clean lip.
    .node("soft_mask", Blur::new("interior").radius(0.0040))
    .node(
        "rounded_mask",
        Levels::new("soft_mask").in_range(0.58, 0.68),
    )
    .node(
        "major_guide",
        Noise::perlin()
            .period(16)
            .octaves(2)
            .seed(seed.wrapping_add(4)),
    )
    .node(
        "middle_guide",
        Noise::perlin()
            .period(64)
            .octaves(2)
            .seed(seed.wrapping_add(5)),
    )
    .node(
        "fine_guide",
        Noise::value().period(512).seed(seed.wrapping_add(6)),
    )
    // The finest damage walks its own guide. A value noise this close to the
    // texel grid speckles the threshold, and the distance transform then turns
    // every speckle into a dot out in the skirt.
    .node(
        "fine_damage_guide",
        Noise::perlin()
            .period(256)
            .octaves(2)
            .seed(seed.wrapping_add(20)),
    )
    // 1294202780 / 1294202882 / 1294202973: three parallel damage scales. The
    // source chips the *mask*, before anything bevels it, so a bite carries its
    // own rounded edge instead of being carved out of a finished height.
    .node(
        "major_cut",
        Blur::slope("soft_mask", "major_guide")
            .radius(0.0030)
            .steps(32)
            .slope_mode(SlopeMode::Min),
    )
    .node(
        "middle_cut",
        Blur::slope("soft_mask", "middle_guide")
            .radius(0.0016)
            .steps(32)
            .slope_mode(SlopeMode::Min),
    )
    .node(
        "fine_cut",
        Blur::slope("soft_mask", "fine_damage_guide")
            .radius(0.0008)
            .steps(32)
            .slope_mode(SlopeMode::Min),
    )
    .node("cut", m(Min, "major_cut", m(Min, "middle_cut", "fine_cut")))
    .node(
        "chipped",
        Mix::new(
            "soft_mask",
            "cut",
            Clamp::new(m(Mul, p("edge_damage"), 0.8)),
        ),
    )
    // An exact distance transform over a jagged boundary throws radial ridges
    // out into the skirt, so the chips are smoothed at the few-pixel scale
    // before the profile measures them. The bites survive; the comb does not.
    .node(
        "damaged_mask",
        Levels::new(Blur::new("chipped").radius(0.0028)).in_range(0.58, 0.68),
    )
    // 1294177920: the bevel is a distance composition. Measuring the distance
    // on both sides of the silhouette gives a signed coordinate, so one curve
    // states the whole profile: the plateau, the shoulder, and the skirt that
    // carries the stone down into the joint instead of dropping off a cliff.
    .node(
        "inner_distance",
        Distance::new(Invert::new("damaged_mask")).range(PROFILE_REACH),
    )
    .node(
        "outer_distance",
        Distance::new("damaged_mask").range(PROFILE_REACH),
    )
    .node(
        "signed_distance",
        m(
            Add,
            0.5,
            m(Mul, 0.5, m(Sub, "inner_distance", "outer_distance")),
        ),
    )
    // The jump-flood distance is exact, so a boundary quantised to whole texels
    // hands the curve a coordinate that wobbles half a texel, and the curve's
    // steep middle multiplies that into a comb of stripes down the flank. The
    // blur belongs here rather than on the finished profile: smoothing the
    // coordinate removes the wobble and the curve then lays its own knee on a
    // clean field, where smoothing the profile afterwards would round the arris
    // the knee is there to make.
    .node("signed_smooth", Blur::new("signed_distance").radius(0.0024))
    // The reference bevel is not one width: a stone's high side drops through a
    // wide gentle chamfer while its low side or a chipped run meets the joint
    // in a few steep pixels. Scaling the signed coordinate about the
    // silhouette before the curve reads it varies the facet from about 14 px
    // (steep) to 44 px (gentle), with a per-stone factor so some stones sit
    // sharper all round and a coarse noise so one side differs from the next.
    .node(
        "bevel_noise",
        Noise::perlin()
            .period(32)
            .octaves(2)
            .seed(seed.wrapping_add(21)),
    )
    .node("bevel_stone", Math::unary(Fract, m(Mul, "stone_id", 13.0)))
    .node(
        "bevel_k",
        m(
            Mul,
            remap("bevel_noise", 0.55, 1.65),
            remap("bevel_stone", 0.80, 1.25),
        ),
    )
    .node(
        "signed_mod",
        Clamp::new(m(Add, 0.5, m(Mul, m(Sub, "signed_smooth", 0.5), "bevel_k"))),
    )
    .node(
        "base",
        // Measured on the reference: the chamfer runs from about 10 px outside
        // the silhouette to 10 px inside at a nearly constant slope, and the
        // top is flat past the arris. In signed units (reach 24.6 px each way)
        // that is a straight line from 0.18 to 0.70 with the knee at 0.70;
        // collinear points hold a straight run in the monotone cubic. The foot
        // was at 0.10, which laid the skirt so far out that the profile's
        // steepest run fell outside the silhouette and the facet read soft; the
        // reference's is steeper over a shorter reach. The coordinate itself is
        // untouched, so the rim, the contact line and the sand bleed read the
        // same window they were tuned on.
        Curve::new("signed_mod").points([
            [0.00, 0.000],
            [0.18, 0.000],
            [0.38, 0.385],
            [0.54, 0.692],
            [0.66, 0.923],
            [0.70, 1.000],
            [0.76, 1.000],
            [1.00, 1.000],
        ]),
    )
    // 1294208390 / 1295112500: a linear gradient stamped per tile at a random
    // rotation, its luminance randomised per tile, multiplied into the stone
    // mask. Reading the reference back tells you exactly what that produces:
    // every stone's low side sits close to one datum — 0.45 / 0.47 / 0.50 at
    // the tenth, the median and the ninetieth of the stones — and what varies
    // per stone is how far its high side climbs above it, 0.50 on the flattest
    // and 0.88 on the proudest. So the lift is a floor plus a per-stone rise,
    // not a scale on a per-stone height: a scale shears whole corners down into
    // the joint, which is what the measured silhouettes showed.
    // The ramp is not clamped. At 1.8 it saturated over most of the cell, and a
    // clamp is a crease: the level line where it bites is straight, runs at the
    // stamp's own angle, and crosses every stone as the diagonal band the review
    // found. Dividing by `|cos| + |sin|` instead makes the ramp span exactly one
    // cell whatever angle it was stamped at, so it reaches nought and one at the
    // cell's own corners and never needs a rail.
    .node("slope_angle", m(Mul, "turn", 1.0))
    .node("slope_cos", Math::unary(Cos, "slope_angle"))
    .node("slope_sin", Math::unary(Sin, "slope_angle"))
    .node(
        "slope_reach",
        m(
            Add,
            Math::unary(Abs, "slope_cos"),
            Math::unary(Abs, "slope_sin"),
        ),
    )
    // The gamma is what anchors that low side. A straight ramp puts half of
    // every stone's rise below its own centre, so a proud stone floats a tenth
    // of a unit above a flat one all the way round and the datum is no longer
    // one datum. Raised to 1.4 the ramp is nearly nothing over the low third of
    // a stone whatever its rise is, and climbs from there; measured, that holds
    // the spread of the low sides to 0.035 against the reference's 0.029, where
    // the straight ramp gave 0.066.
    .node(
        "slope_ramp",
        Levels::new(m(
            Add,
            0.5,
            m(
                Div,
                m(
                    Add,
                    m(Mul, m(Sub, "x", 0.5), "slope_cos"),
                    m(Mul, m(Sub, "y", 0.5), "slope_sin"),
                ),
                "slope_reach",
            ),
        ))
        .gamma(1.40),
    )
    .node("lift_id", Math::unary(Fract, m(Mul, "turn", 31.0)))
    // How far this stone's high side climbs above the datum: 0.05 of height on
    // the flattest, 0.36 on the steepest, which is the reference's own spread.
    // Skewed, because that spread is: the reference's median stone rises a
    // seventh of the way to its proudest one's rise and its ninetieth twice as
    // far as its median, which a flat hash cannot deliver.
    .node(
        "lift_norm",
        Levels::new("lift_id").gamma(1.15).out_range(0.08, 0.95),
    )
    .node(
        "stone_lift",
        m(
            Add,
            0.266,
            m(
                Mul,
                "lift_norm",
                m(Mul, m(Mul, p("sloping"), 2.25), "slope_ramp"),
            ),
        ),
    )
    .node(
        "warped_lift",
        GradientWarp::new("stone_lift", "warp_guide").amount(0.00035),
    )
    // A tilted plate is not a cobble. Past the arris the reference's top keeps
    // climbing gently for another forty to sixty pixels, and it climbs less in a
    // corner than along a side: that is what makes a stone read as a rounded
    // object instead of a chamfered tile. A wide Gaussian of the silhouette is
    // exactly that dome for one plane — a half at the boundary, two thirds ten
    // pixels in, and still rising at sixty — and because it is a blur of the
    // ownership mask rather than a function of one coordinate it rounds toward
    // the stone's own centre wherever the plan is wider or narrower.
    // The window is not 0.5 to 1: a blur this wide reads several stones at once,
    // so over a bed that is three quarters stone the field never comes near
    // either rail, and taken raw the dome was a third of the rise it should be.
    // Measured against the reference's own radial profile — which climbs 0.070
    // between ten and fifty pixels inside the crossing — 0.60 to 1.00 through an
    // amplitude of 0.17 is that climb, front-loaded the same way.
    .node("pillow_blur", Blur::new("damaged_mask").radius(0.042))
    .node("pillow", Levels::new("pillow_blur").in_range(0.60, 1.00))
    .node(
        "sloped",
        m(Mul, "base", m(Add, "warped_lift", m(Mul, "pillow", 0.170))),
    )
    // Separate major relief, middle grain, sparse pits and fine granulation.
    // The major relief is one gentle undulation and no more: at period 32 over
    // three octaves it laid sixteen- and thirty-two-pixel lumps across every
    // top, and mid-frequency relief is the one thing the reference's pillows
    // have none of.
    .node(
        "broad",
        Noise::perlin()
            .period(16)
            .octaves(2)
            .persistence(0.60)
            .seed(seed.wrapping_add(7)),
    )
    .node(
        "grain",
        Noise::perlin()
            .period(256)
            .octaves(2)
            .seed(seed.wrapping_add(8)),
    )
    .node(
        "pits",
        Levels::new(Voronoi::new().period(256).seed(seed.wrapping_add(9))).in_range(0.13, 0.04),
    )
    .node(
        "micro",
        Noise::value().period(512).seed(seed.wrapping_add(10)),
    )
    .node(
        "texture",
        m(
            Add,
            remap("broad", -0.07, 0.03),
            m(
                Add,
                remap("grain", -0.008, 0.005),
                m(
                    Add,
                    m(
                        Mul,
                        m(Mul, "pits", Levels::new("broad").in_range(0.35, 0.65)),
                        -0.010,
                    ),
                    remap("micro", -0.0012, 0.0012),
                ),
            ),
        ),
    )
    .node(
        "stone_height",
        Clamp::new(m(
            Add,
            "sloped",
            m(Mul, m(Mul, "base", "texture"), p("surface_noise")),
        )),
    )
    // 1294177255: the source splatters a warped blob with full disorder and
    // full size variation, which is why its aggregate reads as gravel and not
    // as a grid. A jittered cellular lattice gives the same thing for two
    // generators: the distance field draws the pebble and the cell field sets
    // its radius, with the sites wherever the jitter put them.
    //
    // The one rule this has to obey is that a pebble must finish well inside
    // its own cell. The distance field is the distance to the *nearest* site,
    // so past the cell boundary it belongs to a different cone: a pebble drawn
    // wider than its cell gets cut off along the Voronoi polygon, which is what
    // made these lumps hexagonal, and the per-cell radius steps across that
    // same boundary, which is what drew sawtooth hairlines between them. Both
    // go away by keeping every radius a small fraction of its cell, so the
    // level sets stay circles and the radius only ever changes where the pebble
    // is already nothing. Coverage then has to come from three lattices rather
    // than from one lattice of fat pebbles, which is also how a real bed is
    // graded: a few coarse stones, more medium, gravel between them.
    .node(
        "pebble_coarse",
        stone_dome(seed.wrapping_add(11), 32, 0.095, 0.255, 0.35, 1.00),
    )
    .node(
        "pebble_medium",
        stone_dome(seed.wrapping_add(21), 64, 0.105, 0.225, 0.40, 0.85),
    )
    .node(
        "pebble_fine",
        stone_dome(seed.wrapping_add(34), 128, 0.090, 0.235, 0.40, 0.45),
    )
    // The fourth lattice is in the bed as well as in the chips. Counted on the
    // reference there are about 2200 separate lumps per megapixel of joint with
    // a median area of three texels; the three graded lattices above are 64, 32
    // and 16 pixel cells and between them they could only deliver half that
    // count at a median of ten. The eight-pixel lattice is the gravel between
    // the stones, and it is what makes the bed read as loose material rather
    // than as a rippled floor.
    //
    // The two fine grades are taken taller here than the nodes themselves are,
    // and the scale lives in this expression rather than in their amplitudes
    // because `chip_mask` reads the same two nodes and cuts them at a fixed
    // window: raising the amplitude there would widen every coloured flake as
    // well. The coarse grade is untouched, so the tallest thing in the bed is
    // still 0.14 above it and still a clear step under the lowest stone toe.
    .node(
        "aggregate",
        m(
            Max,
            m(Mul, "pebble_chip", 1.40),
            m(
                Max,
                "pebble_coarse",
                m(Max, "pebble_medium", m(Mul, "pebble_fine", 1.35)),
            ),
        ),
    )
    // A fourth lattice that only the chips read. Measured, the reference's
    // flakes have a median diameter under five pixels and three quarters of them
    // are under eight: the population is dominated by small ones, and the three
    // lattices the height is graded on cannot deliver that count because their
    // cells are 64, 32 and 16 pixels. Eight-pixel cells at a low amplitude give
    // four- to six-pixel flakes in the numbers the reference has. It is kept out
    // of `aggregate` on purpose — this is chipping, not a fourth grade of stone
    // for the height to raise.
    .node(
        "pebble_chip",
        stone_dome(seed.wrapping_add(47), 256, 0.100, 0.240, 0.40, 0.30),
    )
    // Reference pebbles crowd the toe of the bevel and thin out mid-joint.
    .node(
        "toe",
        Levels::new("outer_distance")
            .in_range(0.62, 0.02)
            .out_range(0.90, 1.0),
    )
    // Perlin values crowd the middle, so the undulation is stretched before it
    // is scaled, and skewed as well, because the reference's bed is: its low
    // tail is twice its high one, so the bed mostly lies near its own top level
    // and dips into hollows rather than heaping into mounds. What the skew is
    // not allowed to do is be large: see `bed` for why a swing of more than a
    // few hundredths here is a joint whose width the eye can see change.
    .node(
        "grout_swell",
        Levels::new(
            Noise::perlin()
                .period(32)
                .octaves(2)
                .seed(seed.wrapping_add(18)),
        )
        .in_range(0.34, 0.66)
        .gamma(0.55),
    )
    .node(
        "grout_ripple",
        Levels::new(
            Noise::perlin()
                .period(128)
                .octaves(2)
                .seed(seed.wrapping_add(19)),
        )
        .in_range(0.36, 0.64),
    )
    // The bed's own floor, and what ownership is decided against. Two rules
    // govern the amplitudes here, and breaking either one is what the last pass
    // did. It has to stay inside 0.08..0.22 so the floor is the reference's
    // floor and every stone toe stands clear of it; and its swing has to be
    // small, because the chamfer it meets falls at about a thirtieth of the
    // range per pixel, so every 0.03 of bed noise moves the crossing a pixel.
    // The ±0.145 swell that used to stand here moved it nine pixels each way
    // and the joint's width swung by eighteen — the ballooning the review saw.
    //
    // A `crevice` term used to subtract 0.135 wherever a texel was within seven
    // pixels of a stone and a grunge gate allowed it. In a narrow joint that is
    // the whole joint, so the stone claimed it outright; in an open corner it
    // was nothing, so the sand claimed that. Between the two the joint colour
    // broke into islands. Gone: a joint is deep because it is a joint, not
    // because of how close the nearest stone happens to be.
    //
    // The aggregate below crowns the bed, and deliberately after the node
    // ownership reads: a flake leaning on a chamfer does not turn that chamfer
    // into sand.
    .node(
        "bed",
        m(
            Add,
            p("grout_height"),
            m(
                Add,
                remap("grout_swell", -0.090, 0.025),
                m(
                    Add,
                    remap("grout_ripple", -0.018, 0.018),
                    remap("fine_guide", -0.008, 0.008),
                ),
            ),
        ),
    )
    // Crowned, not filled: a pebble lifts the bed by up to 0.14, which leaves
    // the tallest of them a clear step under the lowest stone toe.
    .node(
        "grout",
        m(Add, "bed", m(Mul, m(Mul, "aggregate", "toe"), 0.14)),
    )
    // 1294186292: height composition and one shared ownership mask.
    //
    // Ownership is where the stone's profile crosses the bed — but against a
    // *nominal* stone rather than this stone's own top, and that is the whole
    // difference between a joint you can follow round a stone and a joint that
    // pinches and balloons. Read bare, `stone_height` carries the per-stone
    // lift, so the crossing sat four pixels outside a proud stone and nearly
    // two inside a flat one, and the colour joint's width swung by seven
    // pixels across one junction. `base` alone is the profile every stone
    // shares, so a fixed fraction of it is a contour at a near-constant offset
    // from the silhouette: measured, one and a half pixels outside it, varying
    // by under three with the per-stone bevel. The bias is small and negative,
    // which puts that contour just inside the toe rather than out in the sand.
    //
    // The `Min` is what keeps the mask honest when the bed really does win. A
    // stone whose own top has gone under — which is what the grout control does
    // at the head of its range — is claimed by the sand on its real height, not
    // on the profile it would have had. Both terms fall as the grout rises, so
    // the exported mask stays monotone in it.
    .node(
        "claim",
        m(
            Sub,
            m(Add, m(Min, "stone_height", m(Mul, "base", 0.36)), -0.020),
            "bed",
        ),
    )
    .node("stone_mask", Levels::new("claim").in_range(-0.009, 0.009))
    // A hard maximum creases where the skirt meets the grout, and the crease
    // reads as a drawn line in the normal map, so the two surfaces are joined
    // over a band the way the source's Height Blend joins them. Written as the
    // exact maximum plus a quadratic bump rather than as the usual smooth
    // maximum: the bump is exactly zero once the two are further apart than the
    // band, so outside the join the height is bit for bit one surface or the
    // other, and raising the grout can never lower a stone top by a rounding
    // error. It stays monotone in the grout inside the band as well, because
    // the bump sheds at most half of what the maximum gains.
    .node(
        "join",
        Clamp::new(m(
            Add,
            0.5,
            m(Mul, m(Sub, "grout", "stone_height"), 0.5 / JOIN_WIDTH),
        )),
    )
    .node(
        "height",
        m(
            Add,
            m(Max, "stone_height", "grout"),
            m(Mul, m(Mul, "join", Invert::new("join")), JOIN_WIDTH),
        ),
    )
    // ----------------------------------------------------------- shared masks
    // 1294738141: the arrises the source brightens and polishes. The radius is
    // small on purpose: at ten pixels it answered the whole bevel shoulder and
    // put a bright halo inside every stone edge where the reference profile
    // climbs smoothly inward. The matching cavity read is gone with the ring it
    // used to draw: at six pixels it only ever found the arris again, so it
    // thickened the outline instead of finding the dents a stain collects in.
    .node(
        "edge_peak",
        Curvature::peaks("height").radius(0.0028).strength(6.0),
    )
    // 1294738172 / 1294738564: the source's broad per-region shine. This used to
    // be a 64 px high pass of the occlusion, and a high pass of the occlusion is
    // not a patch field at all — the only thing the occlusion knows at that
    // scale is how far the texel is from a joint, so it came out as a smooth
    // ramp that was zero at every toe and full in the middle of every face. It
    // washed the interiors milky, put a dark ring twenty pixels in, and in the
    // roughness drew the flat light ribbon around every stone. A patch a
    // stone-and-a-half across, slid per stone so it never crosses a joint, is
    // what the mask was meant to be: 60 to 150 px smudges, darker and rougher
    // where the stone is dirty, with no relation to the silhouette.
    // Smaller and harder than it was. At period 16 this laid 128-pixel clouds
    // with a gradient for an edge, and three of them over a face is an
    // airbrush: read at full view ours was soft light and dark cloud where the
    // reference is brushwork on a flat charcoal ground. Sixty-pixel patches, a
    // window half as wide, and the face's own mottle summed in before the
    // window so the patch stops on a ragged line rather than on a smooth arc.
    .node(
        "smudge_seed",
        Noise::perlin()
            .period(32)
            .octaves(3)
            .persistence(0.55)
            .seed(seed.wrapping_add(48)),
    )
    .node(
        "smudge_t",
        Levels::new(m(
            Add,
            m(Mul, "smudge_seed", 0.78),
            m(Mul, "mottle_seed", 0.22),
        ))
        .in_range(0.36, 0.64),
    )
    .node(
        "smudge",
        IntensityWarp::new("smudge_t", "stone_id")
            .angle(0.412)
            .amount(0.28),
    )
    // 1294806362 mg_mask_builder_2. The measured profile — linear luminance
    // 0.111 in the stone interior, 0.084 in a sixteen-pixel valley centred on
    // the boundary, 0.122 in the middle of the joint — is an average, and the
    // ring that reproduced it was the wrong construction for it. In the
    // reference the darkening is warm-brown staining that creeps in from some
    // edges and corners and skips others: ten pixels along one side, sixty up
    // the next, none on the third, mottled and hard-edged where it stops. A
    // proximity field added to a grunge and then thresholded draws that, and
    // because the grunge alone never reaches the threshold no stain floats free
    // in the middle of a face.
    .node(
        "edge_band",
        Invert::new(m(
            Max,
            m(Mul, "inner_distance", 1.30),
            m(Mul, "outer_distance", 1.50),
        )),
    )
    // How far the stain can creep. A wide blur of the ownership mask is one out
    // in the joint, still a fifth of the way up sixty pixels inside a face, and
    // lower in a corner than along a side because two joints feed it there —
    // which is where the reference's deepest tongues are.
    .node("stain_far", Blur::new("stone_mask").radius(0.030))
    // Wider than it was. The reference stains a third to a half of a face from
    // one side or one corner; a creep that had run out by sixty pixels could
    // only ever draw a tongue up a toe, which is the small brown blotches
    // hugging the rim the review found. At 0.16 the field is still half up in
    // the middle of a face, so how far a stain reaches is the grunge's business
    // and the distance only tilts it.
    .node(
        "stain_creep",
        Invert::new(Levels::new("stain_far").in_range(0.16, 1.05)),
    )
    // Staining belongs to the stone face. Left free to run out into the joint it
    // painted the sand beside every stone dark brown, which is the same outline
    // the ring drew, so the reach is cut off across the toe: full until five
    // pixels past the silhouette, gone by thirteen.
    .node(
        "stain_side",
        Invert::new(Levels::new("outer_distance").in_range(0.20, 0.55)),
    )
    .node(
        "stain_reach",
        Clamp::new(m(
            Mul,
            m(Max, "edge_band", m(Mul, "stain_creep", 0.92)),
            "stain_side",
        )),
    )
    // 1294838375 fractal_sum and 1294854913 grunge_013: the coarse field decides
    // which sides stain at all, the finer one mottles each tongue and gives it
    // the hard small-scale edge the reference's staining has.
    .node(
        "dirt_grunge",
        Noise::perlin()
            .period(8)
            .octaves(2)
            .persistence(0.5)
            .seed(seed.wrapping_add(22)),
    )
    // Measured on the reference, the staining is not a few large patches but
    // about 1600 separate marks over the repeat, median four pixels across and
    // rarely more than sixteen, clustered where the tongues reach. So the fine
    // field runs down to the finest lattice a 512 diagnostic can still hold.
    .node(
        "stain_grunge",
        Noise::perlin()
            .period(64)
            .octaves(4)
            .persistence(0.70)
            .seed(seed.wrapping_add(36)),
    )
    // Two perlins summed pile up around a half, and a gate that sits at a half
    // everywhere stains every edge equally, which is the ring again. Stretched
    // to span its own range, the gate spends half its area below the level a
    // stain needs even hard against the toe.
    // The weights are swapped from what they were. With the fine field leading
    // at 0.62 the gate was decided at four to sixteen pixels, so a stain was a
    // confetti of small marks wherever the reach was high — the brown blotches
    // at the rim. The coarse field is a 256-pixel lattice: leading at 0.64 it
    // decides *which side of which face* stains, in one connected region a
    // third of a stone across, and the fine field is left to mottle the inside
    // of that region and to fray its boundary.
    .node(
        "stain_gate_raw",
        Levels::new(m(
            Add,
            m(Mul, "stain_grunge", 0.32),
            m(Mul, "dirt_grunge", 0.68),
        ))
        .in_range(0.28, 0.72),
    )
    // And slid per stone, the way the strokes and the smudge are. The coarse
    // field's cells are a stone wide, so read straight it painted one blotch
    // across a stone, its joint and its neighbour — the same mark continuing on
    // the far side of a joint, which is the one thing in the reference's
    // staining that never happens: there the sand is a boundary and each stone
    // is stained on its own account. Offsetting the gate along its own axis by
    // the stone's identity gives every stone an independent draw for no field
    // of its own.
    .node(
        "stain_gate",
        IntensityWarp::new("stain_gate_raw", "stone_id")
            .angle(0.733)
            .amount(0.30),
    )
    // The reach only tilts the odds: it is worth 0.24 of the field hard against
    // the toe and 0.014 in the middle of a face, so where the stain falls is the
    // grunge's decision and how often is the distance's. Weighted much harder
    // the threshold follows the joint instead of the grunge and draws a
    // continuous gutter along every stone, which is the old outline again with a
    // ragged edge; weighted much softer the marks spread evenly over the faces.
    // Measured on this field, the two scans below then stain a seventh of the
    // toe against a fortieth of a face, against the reference's tenth and
    // fortieth.
    .node(
        "stain_field",
        m(
            Add,
            m(Mul, m(Add, 0.06, m(Mul, "stain_reach", 0.94)), 0.18),
            m(
                Add,
                m(Mul, "stain_gate", 0.75),
                // The stone's own fine grain, borrowed to fray the tongue's
                // edge: without it the threshold draws smooth arcs, and a
                // stain that stops on a smooth arc reads as a decal.
                m(Mul, "grain", 0.12),
            ),
        ),
    )
    // Two scans of the one field rather than one. A single hard threshold makes
    // every mark equally dark and the face reads as flaked paint; the reference
    // has a broad, weak, warm veil over the first tongues and only a sparse
    // fraction of it going to real dark brown. The wide scan keeps a little of
    // its own gradient so a veil can fade, the core scan stays crisp.
    // Both scans come down about a sixteenth, which is what turns a tongue up a
    // toe into a stained region: the reference's stained zone takes a third to
    // a half of many faces, and a scan that only fired where the reach was
    // already high could never leave the rim.
    // Both windows are narrower than they were. A scan 0.156 of the field wide
    // is not a scan at all, it is a ramp: read at 3x our stained third was an
    // airbrushed brown cloud with no boundary anywhere on it, where the
    // reference's staining stops on a mottled line the fine grunge frays. Two
    // thirds of the width about the same centre keeps the same area and hands
    // the edge back to the grunge.
    // Both scans come down again, and this time it is coverage that is wanted
    // rather than depth. Counted stone by stone the reference marks *most* of
    // its cobbles — a soft darkening round an edge or into a corner on four
    // stones in five — and only a handful carry a patch you would call a stain;
    // ours fired on a third of them and left the rest spotless, which read as a
    // few splatters on a clean wall. The scans now sit low enough to catch a
    // tongue on nearly every face, and the tints below are cut to match so the
    // extra coverage arrives as tone and not as paint.
    .node(
        "stain_wide",
        Levels::new("stain_field").in_range(0.505, 0.615),
    )
    .node(
        "stain_core",
        Levels::new("stain_field").in_range(0.665, 0.720),
    )
    .node(
        "dirt_mask",
        Clamp::new(m(
            Mul,
            m(Add, m(Mul, "stain_wide", 0.62), m(Mul, "stain_core", 0.38)),
            p("dirt"),
        )),
    )
    // The thin dark line where a stone meets the sand. A window on the signed
    // coordinate just outside where ownership changes hands, so it hugs the toe
    // at whatever width that stone's own chamfer has: three pixels of contact
    // shadow, not a second rim. It moved with the crossing: the profile curve's
    // foot came in from 0.10 to 0.18 and the bed rose, which together put the
    // hand-over 0.06 further along the same untouched coordinate.
    .node(
        "contact",
        m(
            Mul,
            Levels::new("signed_mod").in_range(0.40, 0.35),
            Levels::new("signed_mod").in_range(0.23, 0.28),
        ),
    )
    // The rim, and what makes a stone read as a separate rounded object at all.
    // Removing the flat twenty-five pixel halo left the faces running into one
    // another, because the reference does not have nothing there: it has a thin
    // warm-brown line on essentially every edge, darkest hard against the toe
    // and gone six to fourteen pixels in, about a third darker than the face and
    // half again as warm. It is a narrow, always-present companion to the stain
    // above rather than a replacement for it — the stain is what occasionally
    // swells this into a twenty-to-sixty pixel tongue at a corner.
    //
    // It has to be a falloff and not a window. Read against the profile curve,
    // the height crossing sits near signed 0.30 and the arris near 0.70, so the
    // old window — a straight ramp between 0.50 and 0.70 — was a flat band
    // lying nine to nineteen pixels *inside* the face with a corner at each
    // end, which is the double outline the review saw and not a rim at all.
    // The coordinate is now rescaled about the toe rather than about the middle
    // of the chamfer, and read through a wide range at a gamma above two: full
    // hard against the toe, a fifth of that eight pixels in, nothing by
    // eighteen, and with zero slope where it ends, so there is no inner edge to
    // see. `rim_k` wanders the reach between about nine and twenty-two pixels.
    .node(
        "rim_noise",
        Noise::perlin()
            .period(64)
            .octaves(2)
            .seed(seed.wrapping_add(44)),
    )
    // Measured against the reference, the round this replaces had the falloff
    // both too shallow and far too short. Binned by distance inside the height
    // contour — the same contour in both maps, because the two height profiles
    // agree to a pixel — the reference runs 0.79 of the face's own tone at the
    // contour, 0.89 four pixels in, back to one by ten and a little *over* one
    // out on the shoulder; ours read 0.87 / 0.90 / 0.97 and crept back to one
    // over forty. Baked as a diagnostic the cause was plain: the whole rim had
    // died by signed 0.75, which is where that contour sits, so essentially
    // none of it was reaching the flat of the face. It is not a tighter band
    // that is wanted but a longer one — the reference darkens a good thirty
    // pixels in, gently, and is near-black only over the first four or five.
    //
    // So the window runs the coordinate's whole remaining length: full to four
    // pixels inside the toe, half at nine, a fifth at seventeen, gone by
    // twenty-four, with no slope where it ends. The tint is lighter than the
    // short band's had to be, because the depth now comes from the reach. The
    // gamma is a compromise the construction cannot escape: at 1.25 the line
    // lands on the reference's 0.79 at the contour and is still 0.94 where the
    // reference has come back to 1.02, and at 2.1 it clears by ten pixels and
    // is only 0.84 at the contour. The reference's own recovery overshoots to
    // 1.06 on the shoulder, which is a *lift* and not a shorter rim, and this
    // graph has no business drawing a second ring around every stone to get
    // it.
    .node("rim_k", remap("rim_noise", 0.70, 1.60))
    .node(
        "rim_coord",
        Clamp::new(m(Add, 0.55, m(Mul, m(Sub, "signed_mod", 0.55), "rim_k"))),
    )
    .node(
        "rim_fade",
        Levels::new("rim_coord").in_range(1.000, 0.300).gamma(1.65),
    )
    // Wide, because the reference's rim is near-black on some runs of a toe and
    // barely there on others; it is the tongues of stain that then swell the
    // dark ones into a face. The floor is lifted, though: at 0.30 a third of
    // the wall's toes carried no line at all and those stones ran into their
    // neighbours, where the reference outlines every silhouette.
    // The floor comes up again and the spread narrows with it. Read at full
    // view the net of stones is what makes the reference read as cobbles at
    // all: there is a thin dark line on every silhouette, so the sand never
    // meets a face at its own tone. Ours had the line on most toes and a
    // near-tie on the rest, and a net with gaps in it is not a net.
    .node(
        "rim_strength",
        Noise::perlin()
            .period(32)
            .octaves(3)
            .persistence(0.6)
            .seed(seed.wrapping_add(45)),
    )
    .node("rim", m(Mul, "rim_fade", remap("rim_strength", 0.78, 1.15)))
    // 1294742450: ownership is the height crossing itself, and nothing else.
    // Subtracting the rim from it used to hand a twenty-pixel band of every
    // chamfer to the flat joint tone and to the joint's roughness, which is what
    // drew a halo around each stone and made the joints read three times their
    // width. The stone's colour and roughness now run out over the whole
    // chamfer to the toe, and the joint is what is left between two toes.
    .node("own", Levels::new("claim").in_range(-0.006, 0.006))
    // ------------------------------------------------------------ stone colour
    // 1294732107: a 97-stop gradient over the per-stone identity, overlaid at
    // $Stone_ColorVariation 0.1 so the saturated end never dominates. Measured
    // per stone in the reference: linear luminance 0.1095 ± 0.0136 over
    // 0.073..0.142, and the hue almost fixed at R/B 1.07 with a handful of warm
    // brown stones reaching 1.30. Nine stops state that distribution, scaled so
    // that the multiplicative grain below, whose arithmetic mean is well over
    // one, lands the stone face on the measured 0.1075 rather than half again
    // over it.
    // The spread is measured, and it is narrow. Labelled stone by stone — every
    // face over four thousand texels, in both maps, by the same method — the
    // reference's per-stone mean luminance runs 0.073 / 0.092 / 0.108 / 0.124 /
    // 0.136 at its extremes and deciles for a standard deviation of 0.0125.
    // Stretching this ramp on the strength of a spread measured another way
    // took ours to 0.0246 with a tenth percentile at 0.076, which is the
    // handful of near-black cobbles the review found punching holes in the
    // wall. The stops are compressed to five eighths about the reference's own
    // median, which lands the spread on 0.013; the hues and the one warm brown
    // stop ride through the compression untouched, and the whole ramp is then
    // lifted a fiftieth so the compressed median lands back on the reference's.
    // And the hue rides the ramp rather than sitting flat on it. Binned by
    // luminance the reference's face is R/B 1.31 in its darkest sixth and 1.08
    // to 1.13 everywhere above — and what is actually dark on a face at that
    // level is a whole dark *stone*, not a shaded patch of a pale one, which is
    // why a tint keyed to the face's own tone warmed the middle of the wall and
    // left the darkest sixth at 1.07. The ramp's bottom two stops carry that
    // warmth instead: a charcoal cobble is a dirty one.
    // The compression went too far. Read at full view a wall of stones whose
    // per-stone luminance has a standard deviation of 0.013 is one stone
    // repeated, where the reference plainly has darker stones, lighter stones
    // and a few warm brown ones. The stops are re-expanded about the same
    // median, and asymmetrically: the low tail by 1.15 and the high by 1.40, so
    // the spread comes back to about 0.016 measured on the face without
    // reopening the near-black cobbles the symmetric stretch punched into the
    // wall. Two of the nine stops are warm — R/B 1.34 at the charcoal foot,
    // because a dark cobble is a dirty one, and 1.30 at the seven-eighths mark
    // for the brown stones the reference scatters through its bond.
    //
    // And once more on the low tail, this time with the whole ramp *lifted*.
    // Read at full view against the reference the difference was not in the
    // middle of the distribution, which matched: it was that the reference
    // plainly has a minority of distinctly dark cobbles and ours had none below
    // sixteen per cent under its median. The foot goes to a fifth under the
    // ramp's median, which measures as a tenth percentile a seventh under the
    // face's — the reference's own is an eighth under, and a shade more spread
    // than the measurement asks for is what the eye reads as a bond of separate
    // stones rather than one stone repeated.
    //
    // Every stop is then between two and five per cent *over* where it stood
    // before the staining was widened, and that is not a contradiction: the
    // veil now covers most of a face and takes a ninth of its luminance with
    // it, so a ramp that measured the reference's 0.106 before measures 0.096
    // after. The lift is the veil's own cost handed back, and the face comes
    // out on the reference's mean with the staining included rather than
    // without it. Scaling each stop keeps its chromaticity, so the warm foot
    // and the brown stone at seven eighths ride through untouched. A further
    // fiftieth on top of that pays for the darker toe line in the same coin.
    .node(
        "stone_tint",
        Colorize::new("stone_id").gradient([
            (0.000, [0.1076, 0.1010, 0.0946]),
            (0.125, [0.1153, 0.1100, 0.1047]),
            (0.250, [0.1203, 0.1168, 0.1133]),
            (0.375, [0.1248, 0.1226, 0.1201]),
            (0.500, [0.1296, 0.1278, 0.1258]),
            (0.625, [0.1354, 0.1328, 0.1299]),
            (0.750, [0.1477, 0.1343, 0.1211]),
            (0.875, [0.1445, 0.1425, 0.1405]),
            (1.000, [0.1574, 0.1559, 0.1543]),
        ]),
    )
    // 1294653967 cool body under the 1294656643 warm beige and 1294721014 light
    // cool overlays. One broad patch between a cool and a warm tint, both at
    // luminance one and straddling R/B 1.0, so it spreads hue without moving
    // either the level or the mean the tint above already set; the reference's
    // per-pixel R/B runs 0.95 to 1.40 about a mean of 1.07.
    // Half the swing it had. Read at full view the cool end of this mix was the
    // one thing on the wall that was plainly the wrong hue: whole faces came
    // out slate blue where the reference's ground is neutral charcoal, and a
    // per-stone mean that measures warmer than the reference's does not help if
    // the variance around it crosses into blue. The two ends now straddle
    // neutral at R/B 0.94 and 1.08 rather than 0.90 and 1.12.
    // A touch toward neutral on the cool end, and no more than a touch. Read at
    // full view the wall looked a shade blue beside the reference's charcoal
    // and the cool half of this patch is what does it — but measured, the face
    // was already at R/B 1.12 against the reference's 1.10, so most of what the
    // eye called blue was the stone tint being dark rather than the hue being
    // cold. Pulling the cool end to 0.977 took the face to 1.14 and past the
    // reference on the warm side; 0.949 lands it back on 1.10 with the warm
    // staining now spread over most of the wall included.
    .node(
        "patch",
        Noise::perlin()
            .period(16)
            .octaves(3)
            .persistence(0.6)
            .seed(seed.wrapping_add(23)),
    )
    .node("patch_t", Levels::new("patch").in_range(0.30, 0.72))
    .node(
        "stone_hue",
        Mix::new(
            [0.9730, 1.0050, 1.0255],
            [1.0330, 0.9950, 0.9580],
            "patch_t",
        ),
    )
    // 1294655384 / 1294656580 / 1294717147 / 1294725293: four hand-painted
    // grunge maps. These used to multiply the stone body as well, and that was
    // the single worst thing on the face: read at 2x the whole plateau was a
    // mosaic of four-pixel flat cells, which is crazed paving, not slate. What
    // the reference has no room for on a stone is a *cell mosaic* at four to
    // eight pixels — its face is one- and two-pixel grain, which is finer than
    // any lattice here can lay and is authored from the dissolve hash instead.
    // So these fields now serve the loose bed, the chips lying in it, and the
    // chips' own borrowed grain.
    .node(
        "cell_a",
        Voronoi::new()
            .period(512)
            .seed(seed.wrapping_add(24))
            .output(VoronoiOutput::Cell),
    )
    .node(
        "speck_fine",
        Noise::value().period(512).seed(seed.wrapping_add(26)),
    )
    .node(
        "speck_mid",
        Noise::value().period(512).seed(seed.wrapping_add(27)),
    )
    // A flat Voronoi cell is a polygon, and a grit made only of them reads as
    // crazed paving however fine the lattice. A smooth value noise on the same
    // four-pixel lattice mixed in over the top keeps the grain's contrast and
    // takes the polygons' straight edges out of it. Raw rather than high passed:
    // a lattice this close to the texel grid carries no low frequency to remove.
    .node(
        "speck_hi",
        Noise::value().period(512).seed(seed.wrapping_add(42)),
    )
    .node(
        "speck_broad",
        Noise::perlin()
            .period(8)
            .octaves(2)
            .persistence(0.5)
            .seed(seed.wrapping_add(33)),
    )
    // The one broad band the face keeps: a patch a stone or so across, gentle
    // enough to read as tone rather than as a stain. Its old ±50 % was doing
    // the work the strokes do now and blotched whole runs of stones together.
    .node(
        "mineral_broad",
        Levels::new("speck_broad")
            .in_range(0.20, 0.80)
            .out_range(0.975, 1.025),
    )
    // The three fine fields summed and then high passed over nine pixels. A
    // generator on a four-pixel lattice is not a four-pixel band: flat Voronoi
    // cells are white noise and interpolated value noise is redder still, so a
    // period-512 field lays as much power at eleven pixels as at three, and
    // eleven pixels is what reads as pepper rather than as stone. Taking the
    // low pass off is the only way to move that band without a generator finer
    // than a 512-texel diagnostic bake can resolve. One plane, shared with the
    // grout's own tone, and the sum is high passed once rather than each field
    // separately so it is one plane and not three.
    .node(
        "speck_sum",
        m(
            Add,
            m(Add, "speck_fine", "speck_mid"),
            m(Mul, "cell_a", 0.55),
        ),
    )
    .node("speck_lp", Blur::new("speck_sum").radius(0.0020))
    .node(
        "mineral_grit",
        Levels::new(m(
            Add,
            m(Add, 0.5, m(Mul, m(Sub, "speck_sum", "speck_lp"), 1.55)),
            m(Mul, m(Sub, "speck_hi", 0.5), 0.84),
        ))
        .out_range(0.66, 1.40),
    )
    // 1294725022 / 1294731662: grunge_map_005 patched at 29° and sharpened.
    // The reference's dominant stone-face feature is a brushed grunge: soft
    // gritty light bands twenty to sixty pixels across and two or three hundred
    // long, several to a stone and overlapping, with fine scratches inside them.
    // The angle turns a little from stone to stone. One directional blur can
    // only lay one angle over the whole wall, which is why the last pass read as
    // rain falling straight across the joints; three are laid instead, at 107°,
    // 129° and 151°, and each stone picks one by its own identity. Each is also
    // slid along its own axis by that same identity, so two neighbours that drew
    // the same angle still do not share a stroke across the joint between them.
    // 129° is Designer's warpangle 0.1333 turns in Ashlar's frame, where v runs
    // down the image rows.
    // A smear only takes the field apart along the stroke, so a band's width
    // across it is the seed's own, and the seed therefore has to be broadband
    // or every band comes out the same width. A single lattice will not do: a
    // value noise smeared along one axis draws regular parallel hairlines at
    // exactly its own pitch, and the face read as woven cloth. Three octaves of
    // one perlin lay 32, 16 and 8 pixels at once, which is bands of every width
    // from a stroke down to a scratch; one octave finer than this and so much
    // of the face's power sits in ten-pixel bands that it reads as raked sand.
    // Measured on the reference, the stone face's power per octave is nearly
    // flat, which is what three octaves under one smear give.
    // What a band's width is, stated properly. A perlin of period P lays cells
    // of 2048/P texels and its power peaks at a wavelength of twice that, so a
    // *band* — half a wavelength — is 2048/P pixels across. The reference's
    // bands are fifteen to ninety, which is periods 128 down to 24, and the
    // round this replaces asked for period 32 at persistence 0.62: three
    // quarters of its power sat in the 64-pixel band and the octaves that would
    // have cut that into narrower planes were a fifth of it. The smear then
    // takes a further toll on the fine ones — a directional gaussian of sigma
    // 31 keeps only the wavevectors within about a wavelength's worth of
    // perpendicular, which is a narrower wedge the shorter the wavelength — so
    // by the time the thresholds read the field it was one 64-pixel band and
    // nothing else, and two or three soft lobes is what a face came out as.
    //
    // Three octaves at a persistence near one is the fix, and it is the same
    // number the reference's own spectrum gives: 64-, 32- and 16-pixel bands at
    // comparable weight before the smear and at about two to one to two thirds
    // after it. It is also what makes an edge crisp, because the gradient
    // across a band is the finest octave's and not the coarsest: at 0.90 the
    // field crosses a threshold in a pixel and a half where at 0.62 it took
    // five, and three thresholds that used to stagger over twenty-five pixels
    // now land within seven.
    .node(
        "brush_seed",
        Noise::perlin()
            .period(32)
            .octaves(3)
            .persistence(0.90)
            .seed(seed.wrapping_add(38)),
    )
    // Ninety-five pixels rather than a hundred and twenty. Smeared the length
    // of a stone and a half, every pass ran the whole width of a face and the
    // windows below could only read it as one long wash; the reference's
    // passes stop and start inside a face, several to a stone and overlapping.
    .node(
        "brush_a",
        Blur::directional("brush_seed", 107.0).radius(0.046),
    )
    .node(
        "brush_b",
        Blur::directional("brush_seed", 129.0).radius(0.046),
    )
    .node(
        "brush_c",
        Blur::directional("brush_seed", 151.0).radius(0.046),
    )
    // 1294731400: a directional intensity warp of the colour field by the
    // per-stone identity, one per angle so the slide and the pick are decided
    // by the same number at the same place. The pick has to read the identity
    // directly rather than the slid field, or a stone would show fragments of
    // its neighbour's angle wherever the slide crossed a joint.
    .node(
        "brush_a_slid",
        IntensityWarp::new("brush_a", "stone_id")
            .angle(0.297)
            .amount(0.30),
    )
    .node(
        "brush_b_slid",
        IntensityWarp::new("brush_b", "stone_id")
            .angle(0.608)
            .amount(0.30),
    )
    .node(
        "brush_c_slid",
        IntensityWarp::new("brush_c", "stone_id")
            .angle(0.919)
            .amount(0.30),
    )
    .node("brush_pick", Math::unary(Fract, m(Mul, "stone_id", 5.0)))
    .node(
        "brush_raw",
        Mix::new(
            Mix::new("brush_a_slid", "brush_b_slid", m(Step, "brush_pick", 0.34)),
            "brush_c_slid",
            m(Step, "brush_pick", 0.67),
        ),
    )
    // The face's fine texture, and the thing the last pass got wrong. Smearing
    // a per-texel hash along the stroke and then reading it through a
    // seven-thousandth window gave hard two- and three-pixel ticks all leaning
    // the same way at one of two levels: at 3x it read as fur or as a halftone
    // print, and it covered every face at the same density. Read back at 6x the
    // reference face is the opposite of that — continuous-tone underneath, with
    // *isotropic* one- and two-pixel grit over the whole of it, moderate
    // contrast, the light grains a little more visible than the dark, denser
    // inside a light stroke and weaker in a dark smudge.
    //
    // So the grit is the hash itself, unsmeared. One dissolve is two-tone, but
    // every dissolve hashes the same seed at the same coordinate, so seven
    // thresholds on it count how far down the distribution the texel fell:
    // eight levels, which is a tone and not a pattern.
    //
    // Grit is a texture and never a tone driver, and the round that made it one
    // is the round this replaces. Picking the hash's top sixth out as a separate
    // bright *grain*, gathering those grains into wisps along the brushing and
    // then handing the pair an amplitude near one turned the face into white
    // speckle cloud — frost or lichen on a grey stone, with the strokes
    // invisible underneath it. Read back at 2x the reference's ground is a
    // fairly smooth charcoal carrying low-contrast one- and two-pixel grit: you
    // notice it at 2x and not at 1x, it runs at one density across a whole face,
    // and nothing about it clusters. So there is no pick, no wisp and no
    // amplitude field — one continuous field at a twentieth of the swing it had.
    .node("grit_level", dither_level())
    // Two pixels, so the softened copy is a field of three- and four-pixel
    // clumps rather than a smoothed dither.
    .node("grit_clump", Blur::new("grit_level").radius(0.00155))
    // Mostly the clumps, a little of the hash over them: ±0.48 of a field that
    // is worth a fifth of itself wherever it is read, so the face swings about
    // a tenth of its own luminance texel to texel where it used to swing by a
    // factor of two.
    .node(
        "grit",
        m(
            Add,
            m(Mul, m(Sub, "grit_level", 0.5), 0.62),
            m(Mul, m(Sub, "grit_clump", 0.5), 0.88),
        ),
    )
    // The strokes are the face's dominant feature and they are flat tonal
    // planes, not a wash: broad bands a step lighter than the ground, with
    // crisp straight parallel edges along the stone's own brush direction, laid
    // in two or three overlapping steps and near-uniform inside. The windows
    // are what decide that, and the round this replaces had them eleven, five
    // and three pixels wide over an amplitude the grit then drowned. The plane
    // changes by about a three-thousandth per pixel, so 0.0035 is a one-pixel
    // edge: three stacked thresholds at 0.0035 are three planed facets with a
    // hard rim each, which is what a dry-brush pass or a planed stone face is.
    //
    // The per-stone bias is what varies the coverage rather than the amplitude.
    // A gate on the stack scales how strong the steps are, which at the coverage
    // the thresholds fix reads as the same brushwork everywhere at different
    // opacities; shifting the field under fixed thresholds instead takes a face
    // from a quarter covered to three quarters, which is the reference's spread.
    // The grit goes in first at a window's width, so a plane's rim frays at the
    // grain's own scale rather than following a smooth arc.
    // Nearly twice the spread it had. The field's standard deviation on a face
    // is 0.077, so ±0.030 moved a cut by two fifths of a deviation and every
    // stone in the wall came out with much the same coverage at much the same
    // pitch — read at full view that is combed hair, not brushwork. At ±0.055
    // the low cut runs from a fifth of a face to four fifths of it, which puts
    // plainly bare stones and plainly planed ones in the same bond the way the
    // reference does.
    .node(
        "stroke_bias",
        remap(
            Math::unary(Fract, m(Mul, "stone_id", 19.0)),
            -0.0550,
            0.0550,
        ),
    )
    // The smeared field carries its own low frequency, and a fixed threshold on
    // it is not a threshold at all: over a two-hundred-pixel face the local mean
    // wanders by more than the three cuts are apart, so one stone came out
    // banded from edge to edge and its neighbour perfectly bare — which is what
    // the crops kept showing whatever the amplitudes were set to. Taking the
    // hundred-pixel mean off first lands every cut on the same quantile of every
    // face, so the planes are the same strength everywhere and `stroke_bias` is
    // the only thing that moves the coverage.
    .node("brush_lp", Blur::new("brush_raw").radius(0.045))
    .node(
        "stroke_field",
        m(
            Add,
            m(Add, 0.5, m(Mul, m(Sub, "brush_raw", "brush_lp"), 2.2)),
            m(Add, m(Mul, "grit", 0.0075), "stroke_bias"),
        ),
    )
    // Two light cuts, not three, and far apart — which is the whole difference
    // between a plane and a ramp. Measured on the baked plane the field runs
    // 0.447 / 0.501 / 0.558 at its quartiles for a standard deviation of 0.081
    // and a gradient of 0.0066 a pixel across a band. Two thresholds a distance
    // `d` apart in the field therefore draw their contours `d / 0.0066` pixels
    // apart on the stone, and that distance is the width of the flat step
    // between them: the three cuts at 0.04 apart put three contours inside ten
    // pixels, which no eye reads as anything but one soft ramp however narrow
    // each window is. At the 46th and 86th centiles the two contours are a
    // tenth of the field apart, which is fifteen pixels at the median gradient
    // and twenty-six where the field is gentle — a flat plane with a step at
    // each end, which is what the reference's face is made of.
    //
    // The windows can then be narrow, because a window is only ever `w / grad`
    // pixels of ramp: 0.007 is one pixel where the field is steep and two where
    // it is gentle, and under a pixel is an aliased edge rather than a crisp
    // one.
    .node(
        "stroke_lo",
        Levels::new("stroke_field").in_range(0.4925, 0.4985),
    )
    .node(
        "stroke_firm",
        Levels::new("stroke_field").in_range(0.5915, 0.5975),
    )
    // A brush lays dark passes as well as light ones, but only just: the stack
    // above is what carries the face, and the round that took a quarter of the
    // tone off the field's low half made a mottle out of what should be a
    // ground. One step down, one window wide, and no more.
    .node(
        "stroke_dark",
        Levels::new("stroke_field").in_range(0.4370, 0.4300),
    )
    // The faint fine parallel lines inside a plane. It is the smeared field
    // itself at a low continuous amplitude rather than a fourth threshold: the
    // reference's rulings are a tone that varies along the stroke, never two
    // levels, and the field's own three upper octaves are already that ruling
    // laid at the stone's own angle for no extra plane.
    // Cut near enough in half. A continuous ramp of the very field the cuts
    // read is a ramp that runs *up to* every plane edge and away from it again,
    // so at the old weight it put a five-pixel fade on either side of every
    // step and that fade is most of what read as a cloud rather than a plane.
    .node(
        "stroke_grain",
        m(Mul, m(Sub, "brush_raw", "brush_lp"), 0.20),
    )
    // Ungated the strokes cover every stone equally and the wall reads as a
    // rake through sand at an even pitch. Per stone rather than off the hue
    // patch: the patch is a period-16 cloud, so it scaled the stack *within* a
    // face and the planes faded out across their own interiors.
    .node(
        "stroke_gate",
        remap(Math::unary(Fract, m(Mul, "stone_id", 11.0)), 0.55, 1.30),
    )
    // The amplitudes are what the acceptance is written in: a plane edge has to
    // show a luminance step of an eighth of the local tone inside three pixels,
    // and since the face's colour is this factor times everything else, the
    // step at a cut *is* its amplitude. A sixth at the low cut and a fifth at
    // the firm one, with a dark plane an eighth below the ground; where the
    // firm plane lies inside the low one the two stack, so a face runs from
    // 0.13 under its ground to 0.40 over it — the reference's face spread, and
    // no longer something the soft clouds below have to supply.
    .node(
        "strokes",
        m(
            Add,
            0.898,
            m(
                Add,
                "stroke_grain",
                m(
                    Mul,
                    "stroke_gate",
                    m(
                        Sub,
                        m(
                            Add,
                            m(Mul, "stroke_lo", 0.180),
                            m(Mul, "stroke_firm", 0.235),
                        ),
                        m(Mul, "stroke_dark", 0.130),
                    ),
                ),
            ),
        ),
    )
    // The only anisotropic thing left on the face: a scatter of thin light
    // scratches lying along the brushing, which the reference has a handful of
    // per stone. Drawn as segments rather than smeared out of a noise, so they
    // are lines a pixel wide and not a weave.
    // Read at 3x this is the one band the face was plainly short of. Measured
    // by octave the reference carries 0.0081 at four pixels and 0.0083 at eight
    // against our 0.0059 and 0.0060, and what that missing third *is*, looked
    // at, is hairlines: the reference's faces are ruled with one- and two-pixel
    // streaks along the brushing, more of them dark than light, twenty to sixty
    // pixels long. The smeared field cannot supply them — its finest octave is
    // a thirty-two pixel wavelength and the smear thins that further — so they
    // are drawn, as the scratches already are, and the dark set is a second
    // scatter at a different pitch so the two do not lie on top of one another.
    .node(
        "scratch",
        Scratches::new()
            .count(2601)
            .length(0.030)
            .width(0.00062)
            .angle(129.0)
            .angle_spread(22.0)
            .seed(seed.wrapping_add(49)),
    )
    .node(
        "hairline",
        Scratches::new()
            .count(1764)
            .length(0.016)
            .width(0.00090)
            .angle(129.0)
            .angle_spread(30.0)
            .seed(seed.wrapping_add(56)),
    )
    // One flat amplitude, and a small one. Everything that used to modulate it
    // — the stroke, the wisp, the mottle — made the grit a tone, and a grit
    // that carries tone is the frost the review found. The scratches stay
    // because the reference has a few thin light ones per stone, at a third of
    // the weight they had and no longer keyed to a stroke.
    // The light scratches ride the planes, because a dry brush loads a raised
    // facet and skips the hollow; the dark hairlines do the opposite, so a bare
    // stone is ruled and a planed one is swept clean.
    // And a sixth of the mineral speckle the chips already borrow. Read by
    // octave the face was a quarter short at four and eight pixels — 0.0062 and
    // 0.0063 against the reference's 0.0081 and 0.0083 — and that band is not
    // the dither, which is one texel, nor the planes, which are thirty. It is
    // the stone's own mineral: three- to ten-pixel light and dark flecks over
    // the whole face. The round that banished this field from the body was
    // right about *its* weight and wrong about the field: at ±0.37 four-pixel
    // Voronoi cells are crazed paving, at a sixth of that they are what a
    // broken slate face looks like at 3x, and the plane is already paid for.
    .node(
        "gritted",
        m(
            Mul,
            m(
                Sub,
                m(
                    Add,
                    m(Add, 1.0, m(Mul, "grit", 0.88)),
                    m(Mul, "scratch", m(Add, 0.090, m(Mul, "stroke_lo", 0.215))),
                ),
                m(Mul, "hairline", m(Sub, 0.245, m(Mul, "stroke_lo", 0.095))),
            ),
            Mix::new(1.0, "mineral_grit", 0.22),
        ),
    )
    // 1294731400: the broad patch slid along its own axis per stone, so two
    // neighbours never share a tone across the joint between them.
    .node(
        "broad_warped",
        IntensityWarp::new("mineral_broad", "stone_id")
            .angle(0.8667)
            .amount(0.25),
    )
    // The tonal depth inside a single face: the reference's stones carry darker
    // regions sixty to a hundred pixels across, usually toward one side or one
    // corner, with the grit running through them undimmed. What the reference
    // does *not* have is three of these stacked. With the strokes now carrying
    // the face's tone this one is a whisper under them — the broad staining
    // below is what makes a region of a face read darker, and it is warm and
    // hard-edged where a smudge is grey and soft.
    .node("smudge_tone", remap("smudge", 0.935, 1.00))
    // The continuous tone the grit rides on. Under the reference's grain the
    // face is not flat: it is softly mottled at ten to forty pixels, which is
    // finer than any patch the strokes or the smudge lay and coarser than the
    // grain itself, and without it the grit reads as a dither over a flat
    // field. Three octaves of one perlin from its finest legal lattice up.
    .node(
        "mottle_seed",
        Noise::perlin()
            .period(64)
            .octaves(4)
            .persistence(0.60)
            .seed(seed.wrapping_add(50)),
    )
    // A sixteenth, not a tenth. Read at full view the round this replaces had
    // four multiplicative clouds on one face and the strokes underneath them;
    // the ground the reference's planes stand on is fairly smooth, so the
    // mottle is now only what keeps the grit from lying on a flat field. At
    // ±0.125 it was still half again the step a stroke plane makes, and an
    // isotropic blob that outweighs the planes is what takes the direction out
    // of a face however crisp the planes themselves are.
    .node("face_mottle", remap("mottle_seed", 0.925, 1.075))
    // Everything the face does above the grain, kept as a node of its own so
    // the hue below can read it.
    // Between the grain and the mottle the reference's face carries a band our
    // graph had nothing in: measured by octave it has 0.0141 at two to four
    // pixels and 0.0153 at four to twelve against our 0.0098 and 0.0107, and
    // that missing band is why the face read as grain over a wash rather than
    // as stone. Three octaves at a persistence near one is a nearly flat
    // spectrum from sixteen pixels down to four, which is the band exactly.
    .node(
        "face_fine",
        Noise::perlin()
            .period(128)
            .octaves(2)
            .persistence(0.80)
            .seed(seed.wrapping_add(54)),
    )
    // Two thirds of the swing it had, for the mottle's reason: the band is real
    // and the reference has it, but at ±0.155 this one cloud was worth more
    // than the whole stroke stack and it is isotropic, so it read as the soft
    // blotching the review found and the planes read as its shadow.
    .node(
        "face_cloud",
        Levels::new("face_fine")
            .in_range(0.28, 0.72)
            .out_range(0.900, 1.100),
    )
    .node(
        "face_tone",
        m(
            Mul,
            m(Mul, "broad_warped", "smudge_tone"),
            m(Mul, m(Mul, "strokes", "face_mottle"), "face_cloud"),
        ),
    )
    // The reference's ground is neutral charcoal that turns warm brown wherever
    // it is dark — dirt sits in the low tone and a light pass scrubs it off —
    // while ours came out uniformly cool and clean. So the warmth follows the
    // face's own tone rather than a patch of its own: R/B reaches 1.42 in the
    // darkest ground against the body's 1.07, and a light stroke stays where
    // `stone_hue` put it. Read off the tone before the grain, or the hue would
    // speckle texel by texel; the tint is held at luminance one so it moves
    // only the chromaticity.
    // Binned by luminance the reference is blunt about this: R/B is 1.31 in the
    // darkest sixth of a face and 1.08 to 1.13 in every sixth above it. So the
    // warmth is not a cast over the lower half of the tone, it is a steep thing
    // that only the bottom of it sees — a plain window took our middle sextiles
    // to 1.16 and still left the darkest at 1.10, which is warming exactly the
    // wrong population. A wide window at a gamma above two is a fifth of the
    // tint a quarter of the way down and all of it at the bottom.
    .node(
        "face_dark",
        // The window follows the tone. With the strokes lifting `face_tone` by
        // a twentieth on average and the three clouds under them cut to a
        // tenth, a window that opened at 0.90 now sees only the dark stroke and
        // the bare ground was left cool; 0.97 is the same quarter of the
        // distribution the reference warms.
        Levels::new("face_tone").in_range(0.93, 0.60).gamma(2.60),
    )
    .node(
        "dark_warm",
        Mix::new([1.0, 1.0, 1.0], [1.056, 1.010, 0.948], "face_dark"),
    )
    .node("mineral", m(Mul, "face_tone", "gritted"))
    .node(
        "stone_face",
        m(
            Mul,
            m(Mul, "stone_tint", m(Mul, "stone_hue", "dark_warm")),
            "mineral",
        ),
    )
    // 1294738801: Overlay near-white at 0.5 through the curvature mask, which
    // is the bright arris; and 1294738642: a Copy to a brighter tone through
    // the broad shine patch. The patch used to be the occlusion's high pass,
    // which is a function of the distance to the joint and nothing else, so it
    // brightened every interior and left every toe alone — a milky face with a
    // dark ring in it. Taken off the smudge instead, a bright patch is the
    // clean part of a face and a dark one is the dirty part, and neither knows
    // where the edge is.
    // Softer than it was. Read at full view a handful of stones carried a
    // *light* line along a run of toe where every stone in the reference
    // carries a dark one: the arris highlight and the rim sit within a few
    // pixels of one another, so wherever the rim happened to be weak this won
    // and inverted the outline. It is the rim's companion, not its rival.
    .node(
        "lit",
        Mix::new(
            "stone_face",
            m(Mul, "stone_face", 1.38),
            m(Mul, "edge_peak", 0.42),
        ),
    )
    .node(
        "shine",
        Mix::new(
            "lit",
            m(Mul, "lit", 1.04),
            m(Mul, Levels::new("smudge").in_range(0.46, 0.96), 0.30),
        ),
    )
    // 1294717147 / 1294656580: the dark mineral inclusions, the reference's
    // warmest population — R/B 2.1 at luminance 0.010 against 1.07 for the
    // body. Only the coarse lattice is left: the small ones were the darkest
    // cells of the four-pixel flake field and the glints were its brightest,
    // and a scatter of both over every face is the cell mosaic again at the one
    // size the reference has nothing at. Its dark marks are the elongated ticks
    // below. What is left is drawn from the distance field and gated by the
    // cell hash, not cut
    // out of the cell field itself: a threshold on a flat cell value is a whole
    // Voronoi polygon, and with the pepper gone from over the top of them those
    // polygons stood out on the faces as flat translucent facets.
    .node(
        "pit_cell",
        Voronoi::new()
            .period(128)
            .seed(seed.wrapping_add(30))
            .output(VoronoiOutput::Cell),
    )
    .node(
        "inclusions",
        m(
            Mul,
            Levels::new(
                Voronoi::new()
                    .period(128)
                    .seed(seed.wrapping_add(30))
                    .output(VoronoiOutput::Distance),
            )
            .in_range(0.28, 0.15),
            m(Step, Math::unary(Fract, m(Mul, "pit_cell", 17.0)), 0.86),
        ),
    )
    // The reference face carries small dark ticks, three to twelve pixels,
    // elongated roughly along the brushing and crowded where the face is
    // stained rather than spread evenly over it. Line segments draw them
    // directly: a Voronoi threshold can only make a round speck, and a noise
    // thresholded at this size percolates into blobs whatever the window. The
    // gate is a low scan of the stain field, well under the level the stain
    // itself needs, so the ticks crowd the same tongues the staining runs down
    // and thin out on a clean face.
    // The charcoal smudge and the cloud multiplier that used to stand here are
    // gone with them: both were a period-32 perlin, so both drew a soft blob
    // tens of pixels across in the middle of a face, and that softness is what
    // the reference does not have anywhere. Its low-frequency tone is the
    // brushing and the staining.
    // The window rides up with the two stain scans. `stain_field` gained three
    // hundredths when the distance term's weight went up, so a gate left where
    // it was fired over half again the area it used to and the ticks came out
    // as clusters of black dashes on a third of the faces — printed, at 2x,
    // where the reference's marks are sparse and broken.
    .node(
        "fleck_patch",
        Levels::new("stain_field").in_range(0.60, 0.75),
    )
    // Two layers because one node draws at most one segment per cell of its own
    // lattice. The counts are deliberately not equal: 1600 and 1024 sit on
    // lattices of 40 and 32 cells, so the two scatters beat against one another
    // instead of putting every mark of one beside a mark of the other. They
    // used to be 4096 and 2916, which spread black rice evenly over every face;
    // counted on the reference the marks are far fewer than that and nearly all
    // of them fall inside a stained zone, which is what the gate below does.
    .node(
        "fleck_lines",
        Scratches::new()
            .count(2401)
            .length(0.0034)
            .width(0.0010)
            .angle(129.0)
            .angle_spread(26.0)
            .seed(seed.wrapping_add(39)),
    )
    .node(
        "fleck_lines_b",
        Scratches::new()
            .count(1024)
            .length(0.0020)
            .width(0.0009)
            .angle(129.0)
            .angle_spread(44.0)
            .seed(seed.wrapping_add(41)),
    )
    // Every segment a node draws is the same length and the same darkness, and
    // a face covered in them reads as printed dashes. The fine grain is
    // multiplied through each one so a tick is dark at one end and gone at the
    // other: it varies both how strong a mark is and how long it reads, which is
    // the size spread the reference has and a scatter of capsules does not.
    .node(
        "fleck",
        m(
            Mul,
            m(
                Mul,
                m(Max, "fleck_lines", "fleck_lines_b"),
                remap("speck_hi", 0.28, 1.22),
            ),
            m(Add, 0.50, m(Mul, "fleck_patch", 0.50)),
        ),
    )
    // Warm dark brown rather than black: read back, the reference's darkest
    // face marks are iron staining at R/B near two, not holes.
    .node(
        "stone_color",
        Mix::new(
            "shine",
            [0.0225, 0.0146, 0.0092],
            Clamp::new(m(
                Add,
                m(Mul, "inclusions", 0.10),
                // Three quarters of the weight. Read at 2x every tick was the
                // same near-black as its neighbour and a cluster read as
                // printing; the reference's are graded and most of them are a
                // dark grey mark on the stone rather than a hole in it.
                m(Mul, "fleck", 0.78),
            )),
        ),
    )
    // ------------------------------------------------------------ grout colour
    // 1294273868 / 1294732412 into 1294893879: bnw_spots_2 through a 32-stop
    // warm tan gradient, then replace_color to $Grout_Color.
    //
    // Read back at 10x the reference joint is a per-texel crust: the grain is
    // one and two pixels, which is finer than the four-pixel lattice a 512
    // diagnostic bake may carry, so the per-texel dissolve hash is the only
    // thing that can author it. Discs drawn off two Voronoi lattices, packed or
    // spaced, read as printed polka dots at every radius tried.
    //
    // But a dissolve *between two flat ranges* is a two-level dither, and that
    // is what the bed had become: read at 8x it was cream texels hard against
    // near-black ones with almost nothing in between, where the reference's
    // crust is graded — predominantly mid golden-tan, straw grains and
    // dark-brown gaps as the two minorities. The same hash read as a rank
    // (`grit_level` counts how far down the distribution a texel fell, and
    // costs nothing here because the face already builds it) plus the
    // four-pixel cell tone is triangular instead: most texels land mid, and a
    // texel goes bright only where a high rank falls inside a light cell, which
    // is a grain two to five pixels across rather than one.
    //
    // Coverage is one gentle ripple across the bed and nothing else. Scaling it
    // by a period-8 perlin and again by the distance out of the stone is what
    // starved narrow joints across their whole width.
    .node(
        "sand_density",
        Levels::new("grout_ripple")
            .in_range(0.32, 0.68)
            .out_range(-0.045, 0.045),
    )
    //
    // The weights between the two are what decides whether a grain is a grain.
    // Counted on the reference, the bed's bright texels — its top twelfth by
    // luminance — fall in about thirty-one thousand separate marks with a
    // median area of one texel and a ninety-fifth of three; ours fell in twelve
    // thousand with a ninety-fifth of fourteen and a largest of a hundred and
    // thirty, which is why the same mean and the same quantiles read as spilled
    // plaster rather than as sand. The cause is the cell: a `Cell` value is
    // flat over its whole four-pixel polygon, so at 0.45 of the field a light
    // cell carried every texel in it over the ramp's bright end together and
    // adjacent light cells merged. At 0.22 the cell only tilts the odds and the
    // per-texel rank decides, which is one- and two-pixel grains in the numbers
    // the reference has.
    .node(
        "sand_pick",
        Clamp::new(m(
            Add,
            m(Add, m(Mul, "grit_level", 0.78), m(Mul, "cell_a", 0.22)),
            "sand_density",
        )),
    )
    // The crust is not evenly packed: it clusters, a handful of grains at a
    // time. Two jittered lattices read as whole distance fields rather than cut
    // at a radius give a space-filling clump field whose seams fall on the cell
    // boundaries; a tenth of it on the tone is the clustering, without a second
    // population of blobs to see.
    //
    // Both lattices are four pixels. The second used to be eight, and under a
    // ramp steep enough to carry the reference's spread that lattice was no
    // longer a clustering: its cell boundaries drew a ten-to-twenty-pixel
    // network of dark seams through the crust and its centres cream lumps
    // between them, where the reference's crust is even at that scale and does
    // all of its contrast within two to five pixels. `Distance` is in cell
    // units, so halving the feature size changes where the clumping lands and
    // not the field's distribution.
    .node(
        "grain_a",
        Voronoi::new()
            .period(512)
            .seed(seed.wrapping_add(51))
            .output(VoronoiOutput::Distance),
    )
    .node(
        "grain_b",
        Voronoi::new()
            .period(512)
            .seed(seed.wrapping_add(52))
            .output(VoronoiOutput::Distance),
    )
    .node(
        "sand_clump",
        m(
            Max,
            Levels::new("grain_a").in_range(0.62, 0.10),
            Levels::new("grain_b").in_range(0.62, 0.10),
        ),
    )
    .node(
        "grout_field",
        Clamp::new(m(
            Add,
            m(Mul, "sand_pick", 0.94),
            m(Mul, "sand_clump", 0.06),
        )),
    )
    // Carried at one hue the ramp came out chalk: grey-white grains with black
    // pepper between them, where the reference is ochre. The reference's joint
    // is not one hue — its chromaticity is a function of its luminance, and the
    // function is steep: measured on its own crust the R–B gap runs 0.082 at
    // the dark end and 0.055 at the middle, and G sits a little under the R–B
    // midpoint below the median and a little over it above. The gold is in the
    // grains, but the *brown* is in the shadow between them.
    //
    // Both axes of this ramp are read off the reference's own bed, and the ramp
    // is a histogram match rather than a curve anyone drew. A stop sits at a
    // measured quantile of `grout_field` — recovered by inverting the previous
    // ramp over the deep joint, where nothing downstream touches the bed — and
    // carries the colour the reference's bed has at that same quantile, so the
    // baked bed lands on the reference's mean, spread and dark/mid/bright split
    // together rather than on one of them at a time.
    //
    // Two consequences worth stating. With the per-texel rank carrying four
    // fifths of the field the field is near-uniform rather than piled in the
    // middle, so the stops are spread evenly and it is the *colours* that
    // crowd: fourteen stops from black to cream, two thirds of them under a
    // tenth of a unit of luminance, because that is where two thirds of the
    // reference's bed sits. And the warmth is pushed past the law — the R–B gap by
    // three quarters at the dark end easing to two fifths at the bright one,
    // G's dip under the R–B midpoint by four fifths — because everything
    // downstream dilutes: the chips' neutral tone, the bleed over the toe and
    // the stain all pull a joint texel back toward grey, and because the
    // reference's brown is in the shadow between the grains, where measured
    // R/B runs 1.45 and ours ran 1.19. How far past is measured, not chosen:
    // baked and read back inside the ownership mask the bed came out at R/B
    // 1.46 against the reference's 1.39 with the push at three quarters, so it
    // is three tenths.
    //
    // Two corrections read at full view, both in the top half of the ramp. The
    // bed was pink rather than golden: R/B was right but G sat too far under
    // the R–B midpoint at the bright end, and a warm hue with the green pulled
    // out of it is pink, not ochre. The reference's own mean is (0.155, 0.131,
    // 0.111), where G is 0.845 of R; ours ran 0.83 at the mid and fell to 0.80
    // in the grains. G is lifted toward R from the median up, reaching 0.895 of
    // it at the top stop — the brown between the grains, which is measured and
    // is the whole warmth of the bed, is left exactly where it was.
    //
    // And the bed's body comes down a seventh, with the top of the ramp left
    // where it was. Measured inside the ownership mask the bed's *median*
    // texel ran 0.085 of luminance against the reference's 0.071 while its
    // ninety-fifth ran 0.386 against 0.406: too light in the body and, if
    // anything, short at the top, which is exactly the recipe for a crust that
    // reads washed and chalky. The reference's sand is dark between its grains
    // and the grains then stand out of it; ours was a light bed with lighter
    // grains on it, and no amount of hue makes that read as ochre. The stops
    // from the fifth of the field to the three quarters come down to land the
    // median on the reference's, and the ones above are held so the grains
    // stand further out of the darker bed rather than less far. Twice, in the
    // end: the first pull of a seventh left the median at 0.077 against 0.071
    // and the crust still reading light beside the reference's, so the same
    // stops come down a further thirteenth.
    //
    // R/B eases from 1.40 to 1.36 over the same stops, because 1.40 measured
    // back as 1.403 against the reference's 1.355: the dilution the push was
    // set against is smaller now that the staining beside a joint is a grey
    // veil rather than a rust blotch.
    //
    // What the top of the ramp must *not* do is come down with the body. A
    // first pass dimmed the two brightest stops a quarter on the reading that
    // the grains looked grey-white, and read back the grains were the one thing
    // that had been right: the reference's bed holds 0.406 at its
    // ninety-fifth centile against ours at 0.373, so it is darker than ours in
    // the body and brighter than ours in the grains at once. The grains were
    // never too bright — they were too cool, and warming them is what this ramp
    // does. Dark between, bright on top, is why a crust reads as loose grains
    // rather than as a rippled floor, and the two stops go back up.
    .node(
        "grout_tone",
        Colorize::new("grout_field").gradient([
            (0.000, [0.0000, 0.0000, 0.0000]),
            (0.100, [0.0068, 0.0000, 0.0000]),
            (0.180, [0.0214, 0.0091, 0.0051]),
            (0.240, [0.0326, 0.0183, 0.0129]),
            (0.300, [0.0423, 0.0273, 0.0206]),
            (0.360, [0.0526, 0.0368, 0.0289]),
            (0.420, [0.0644, 0.0477, 0.0383]),
            (0.500, [0.0832, 0.0660, 0.0537]),
            (0.580, [0.1116, 0.0919, 0.0772]),
            (0.660, [0.1553, 0.1326, 0.1142]),
            (0.740, [0.2306, 0.2002, 0.1696]),
            (0.820, [0.3830, 0.3350, 0.2818]),
            (0.900, [0.5670, 0.5010, 0.4170]),
            (1.000, [0.8930, 0.7990, 0.6570]),
        ]),
    )
    // 1294734026 / 1294734019 / 1294734108: the splattered aggregate takes its
    // own tone. The reference's aggregate is not beige gravel — it is flat
    // chips of the cobble stone itself lying in the sand, eight to thirteen
    // pixels across, cool grey-blue against the warm bed, uniformly toned with
    // a crisp edge, and they stand 0.066 above the sand in the reference
    // height. So the chip is the crown of the same aggregate the height raises,
    // cut out by one hard threshold: a flake that stands out of the bed is a
    // flake that shows. Drawing them on a lattice of their own, as a separate
    // splatter, is what made the colour pebbles and the height pebbles two
    // unrelated populations and what let a soft radial ramp read as a bead.
    //
    // The reference crowds its chips where several stones meet, because that is
    // where the loose stuff collects and where there is room for a flake to lie
    // flat. The junction lift adds half a window to the aggregate in an open
    // corner and nothing in an eight-pixel joint, so the threshold is one
    // number and the count still varies with the room.
    .node(
        "chip_junction",
        Levels::new("outer_distance")
            .in_range(0.16, 0.46)
            .out_range(0.0, 0.020),
    )
    // The coarse lattice is out of the chip mask. A threshold of 0.19 against a
    // dome of amplitude 1.0 cuts at 0.955 of its radius, so every coarse pebble
    // showed at its full width on a 64-pixel lattice — 30 to 40 pixel discs, and
    // the widest of them clipped along their own Voronoi polygon into elongated
    // blobs. The reference's chips are 8 to 20 pixels, roundish, and there are
    // many small ones. That is the middle and fine domes exactly: 32- and
    // 16-pixel cells carrying radii well inside them, which is also the
    // condition that keeps a level set a circle rather than a polygon. The
    // coarse pebbles stay in the height, where a 40-pixel stone lying in the bed
    // is right; they simply do not take the chip's own tone any more.
    //
    // The window drops with them. Against amplitudes 0.85 and 0.45 a cut at
    // 0.140 is 0.96 and 0.90 of the two radii, so the discs come out at their
    // full size and the coverage the coarse ones carried is made up by the
    // count. It stays 0.04 wide, a tenth of the field's slope here, so the edge
    // is sub-pixel and the disc is uniformly toned inside it.
    .node(
        "chip_mask",
        Levels::new(m(
            Add,
            m(Max, "pebble_chip", m(Max, "pebble_medium", "pebble_fine")),
            "chip_junction",
        ))
        .in_range(0.100, 0.140),
    )
    // Slight tone variation between chips, off a field whose features are twice
    // a chip wide, so a single chip is very nearly one colour and no hash seam
    // ever crosses one.
    .node("chip_id", Levels::new("middle_guide").in_range(0.28, 0.72))
    .node(
        "chip_tone",
        m(
            Mul,
            // A narrow ramp: the reference's chips are one material seen at one
            // angle, all within a stop of each other. The old ramp ran from
            // 0.089 to 0.254 and scattered light and dark flakes through the
            // joint as if they were three different stones.
            // Cobble grey, and a sixth lighter than it was. Measured, the
            // reference's chips sit at 0.140 where ours were at 0.120, and with
            // B above R, which against a bed that is now ochre read as blue
            // flakes. Not neutral either: the reference's joint holds R/B 1.33
            // through the deciles the chips occupy, so its flakes are dusted
            // with the bed they lie in. R/B 1.09 is that dusting — still grey
            // beside a sand at 1.4, and no longer a cold hole in it.
            Colorize::new("chip_id").gradient([
                (0.000, [0.1360, 0.1290, 0.1250]),
                (0.500, [0.1660, 0.1575, 0.1525]),
                (1.000, [0.2000, 0.1898, 0.1838]),
            ]),
            // A trace of the stone's own grit so a chip reads as broken stone
            // and not as a sticker, and no more: the reference's chips are flat.
            Mix::new(1.0, "mineral_grit", 0.28),
        ),
    )
    .node(
        "grout_color",
        Mix::new("grout_tone", "chip_tone", "chip_mask"),
    )
    // -------------------------------------------------------------- composite
    // Sand does not stop at the height crossing. In the reference it runs on up
    // the lower chamfer and a little onto the top — golden grains at a density
    // of 0.089 four to eight pixels outside the silhouette against 0.026 well
    // inside it — while ours stopped dead at the toe and made the joints read
    // narrow and beaded. The bleed thins the stone's claim over the chamfer, so
    // what crosses the toe is the bed itself, grains and near-black gaps
    // together, and the tone there comes out at the measured 0.095 rather than
    // at the bed's own 0.155. It cannot draw the old halo for two reasons: the
    // field it lets through is bimodal per texel rather than flat, and a coarse
    // mottle decides how far up each stretch of skirt the sand climbs, so some
    // sides are buried and others are clean.
    .node(
        "bleed",
        m(
            Mul,
            // Hard against the toe, not a haze over the whole chamfer. The band
            // used to ramp from ten pixels inside the silhouette to eight
            // outside, so eighteen pixels of every stone carried a quarter of a
            // bimodal crust: peppered brown, which is what made the joint read
            // ragged and its edge look eaten. Seven pixels, and the sand climbs
            // the toe instead of hazing the facet.
            Levels::new("signed_mod")
                .in_range(0.62, 0.48)
                .out_range(0.0, 0.21),
            Levels::new("dirt_grunge")
                .in_range(0.34, 0.66)
                .out_range(0.45, 1.00),
        ),
    )
    .node("own_color", m(Mul, "own", Invert::new("bleed")))
    // 1294732467: the ownership composite, grout underneath.
    .node(
        "color_own",
        Mix::new("grout_color", "stone_color", "own_color"),
    )
    // 1294800434: $Stone_DirtColor through the mask builder. Designer soft
    // lights it over sRGB-encoded values; Ashlar's SoftLight is Pegtop and runs
    // in linear, where a near-black source squares the backdrop and takes it
    // four fifths down instead of a quarter, so this mixes toward the same rust
    // and carries the same weight. What the weight is set by is the measured
    // rim: luminance 0.083 against 0.113 and R/B 1.16 against 1.07, three
    // pixels inside the stone boundary.
    // 1294800434: $Stone_DirtColor through the mask builder. Written as a tint
    // the stain multiplies by rather than a colour it mixes toward: a mix to a
    // flat brown takes two thirds of the face's own texture out with it and the
    // patch then reads as paint, where the reference's stained patches keep
    // every streak and speck of the stone under them and only turn warm and
    // dark. The factor is the measured stained toe against the clean face —
    // luminance a third of it, R/B 2.1 against 1.07.
    // One tint over the blended mask could only ever draw one depth of stain,
    // and the depth it drew was the average of two populations the reference
    // keeps apart: a broad warm veil over a third of a face, and inside it a
    // small dark core. Averaged, ours came out as a mild brown wash everywhere
    // the stain reached and nothing anywhere was as dark as the reference's
    // cores, which are near-black brown against the stone. Two tints over the
    // two scans, multiplied, say both at once for no field of its own: the
    // veil is *lighter* than the single tint was and the core, which sees both,
    // lands at a sixth of the face's luminance at R/B 2.3 — the reference's own
    // darkest face population.
    //
    // Both tints are desaturated and lifted hard from what they were. At R/B
    // 1.38 on the veil and 2.17 where the two multiplied, this was rust — dried
    // blood on grey slate, in a few saturated blotches — where the reference's
    // staining is a nearly neutral charcoal-brown that only darkens. The
    // measured chromaticity of a stained face there is R/B 1.19 against 1.07
    // for a clean one: a tenth of a turn warmer, not double. The pair now
    // multiplies to about [0.57, 0.53, 0.50] at the core, which is R/B 1.14 at
    // a little over half the face's luminance, and the veil alone is a fifth
    // down at R/B 1.04 — a shadow on the stone rather than a colour on it. Both
    // are a little lighter and a little nearer neutral than the first attempt
    // at this, which measured the wall a ninth dark and a fortieth warm of the
    // reference once the coverage was widened to most of the stones.
    //
    // Both go through the ownership mask, which is what keeps the stain on the
    // stone the way its own construction intends. `stain_side` stops the reach
    // thirteen pixels past the silhouette, and that is thirteen pixels of sand:
    // the tint flattened them into smooth brown clumps lying on a granular bed,
    // which is the one thing in the joint that does not read as loose material.
    .node(
        "stain_veil",
        m(Mul, m(Mul, "stain_wide", p("dirt")), "own_color"),
    )
    .node(
        "stain_deep",
        m(Mul, m(Mul, "stain_core", p("dirt")), "own_color"),
    )
    .node(
        "dirt_tint",
        m(
            Mul,
            Mix::new([1.0, 1.0, 1.0], [0.815, 0.800, 0.787], "stain_veil"),
            Mix::new([1.0, 1.0, 1.0], [0.700, 0.663, 0.635], "stain_deep"),
        ),
    )
    // The rim as a tint for the same reason the stain is one: it has to keep
    // every hairline and stroke of the stone under it and only turn warm and
    // dark, or the silhouette comes back as the painted outline it was. Through
    // the crisp ownership rather than the bled one, so a toe buried in sand
    // still darkens — in the reference the grains packed against a stone are
    // the darkest part of the joint.
    // Darker than it was, because the first few pixels of the toe are where the
    // net lives: at 0.500 the line read a third down from the face where the
    // reference's reads a half down over its first three or four pixels. The
    // hue is where it was — this is a shadow with dirt in it, not the stain.
    .node(
        "rim_tint",
        Mix::new([1.0, 1.0, 1.0], [0.400, 0.335, 0.284], m(Mul, "rim", "own")),
    )
    .node(
        "color_dirt",
        m(Mul, m(Mul, "color_own", "dirt_tint"), "rim_tint"),
    )
    // A chip crowded against the toe is lit, not shadowed, so the contact line
    // steps over the aggregate instead of scoring through it.
    .node(
        "contact_line",
        m(Mul, "contact", Invert::new(m(Mul, "chip_mask", 0.75))),
    )
    .node(
        "color_contact",
        Mix::new(
            "color_dirt",
            [0.0180, 0.0121, 0.0075],
            // Lighter than it was: the rim above now carries the toe, and the
            // two together drew a black outline.
            // Back up by four fifths, and it is the side of the toe it falls on
            // that earns it. `contact` is a window on the signed coordinate
            // between 0.28 and 0.35 with the height crossing at 0.30, so nearly
            // all of it lies in the *sand*: it is the grains packed in shadow
            // against a stone, which in the reference are the darkest thing in
            // the joint and are half of why the net of stones reads at full
            // view. The rim does the stone side; without this the sand came up
            // to a stone at its own bright tone and the line was one-sided.
            m(Mul, "contact_line", 0.36),
        ),
    )
    // 1294898712, sharpen at $Color_Sharpness 0.5, written as an unsharp mask
    // because the crate has no sharpen node. Half the reference's stone-face
    // power sits under five pixels against a twentieth of ours, so the highest
    // band is exactly where this material was short and the plane pays.
    //
    // It is taken through the ownership mask, so the joint is left alone. The
    // sand is authored a grain at a time already, and an unsharp over a
    // per-texel field is not a sharpening: it is a contrast expansion of about
    // 1.8 about the local mean, which drove a fifth of the joint into the black
    // clamp and another eighth into the white one and left the quantile curve
    // wrong at both ends at once. The stone face, whose finest real detail is a
    // few texels across, still gets the whole of it.
    .node(
        "color",
        Clamp::new(m(
            Add,
            "color_contact",
            m(
                Mul,
                m(
                    Sub,
                    "color_contact",
                    Blur::new("color_contact").radius(0.0011),
                ),
                m(Mul, "own_color", 0.85),
            ),
        )),
    )
    // --------------------------------------------------------------- roughness
    // The source's roughness chain is short and its numbers are exposed, so it
    // transcribes almost literally: one flake field, a per-stone shine taken off
    // through the AO high pass, the arris taken off through the curvature, and
    // the dirt added back. Measured against the reference, that profile is 0.486
    // in the stone interior, 0.594 at the toe and 0.641 in the joint beside it.
    // Two additions the source's chain does not have but its map plainly does:
    // the brushing, which scores the roughness with the same strokes and
    // scratches the base colour carries, and a scatter of bright elongated
    // slivers five to twenty pixels long at every angle, clustered in the same
    // patches the dark ticks fall in. The dark round dots between them are the
    // inclusions taken back out again, so a dark speck and a smooth speck are
    // the same grain rather than two unrelated scatters.
    .node(
        "cell_r",
        Voronoi::new()
            .period(512)
            .seed(seed.wrapping_add(32))
            .output(VoronoiOutput::Cell),
    )
    .node("rough_field", m(Min, "cell_r", "cell_a"))
    .node(
        "sand_dither_band",
        m(
            Sub,
            Blend::new(BlendMode::Dissolve, 0.0, 1.0).opacity(0.70),
            Blend::new(BlendMode::Dissolve, 0.0, 1.0).opacity(0.25),
        ),
    )
    .node(
        "shards",
        Scratches::new()
            .count(4096)
            .length(0.0075)
            .width(0.00085)
            .angle(129.0)
            .angle_spread(70.0)
            .seed(seed.wrapping_add(40)),
    )
    // 1294860324 histogram_range(range $Roughness_Noisiness, position
    // $Roughness_Base): the field's median lands on the exposed control, and the
    // span is what puts the reference's 0.35..0.68 across a stone face.
    // Inside a face the reference's roughness is a mottled mid-dark field with
    // the same per-texel grit the colour carries running over it, plus the
    // shard clusters. Measured, its one-pixel high pass inside a stone is 0.078
    // against 0.022 for a mottle alone, so the grit is most of the fine
    // structure there and the cloudiness the review saw was its absence.
    .node(
        "rough_mottle",
        Levels::new(
            Noise::perlin()
                .period(64)
                .octaves(4)
                .persistence(0.60)
                .seed(seed.wrapping_add(46)),
        )
        .in_range(0.28, 0.72),
    )
    .node(
        "stone_rough_base",
        m(
            Add,
            m(
                Add,
                // The exposed control is the joint's level, and the face sits
                // below it: measured on the reference the interior of a stone
                // is 0.491 against 0.641 in the sand beside it, and everything
                // added below is one-sided, so the offset is taken here rather
                // than by moving a control the grout reads as well.
                m(Sub, p("roughness"), 0.136),
                m(Mul, m(Sub, "rough_mottle", 0.5), 0.100),
            ),
            m(
                Add,
                m(
                    Add,
                    // The same grit the colour carries, at the same subordinate
                    // amplitude and with the same wisp gone: in the reference's
                    // roughness the face is a clean mid field with fine grain
                    // over it, and the dark cloud the review saw was this term
                    // running at a sixth of a roughness unit through a mask
                    // that gathered it.
                    m(
                        Add,
                        m(Mul, "grit", 0.115),
                        // The same eight- to sixteen-pixel cloud the colour
                        // rides: measured by octave the reference's roughness
                        // carries 0.033 at four to twelve pixels where ours
                        // carried 0.029, and a map that agrees with the colour
                        // about where the face is open costs nothing here.
                        m(Mul, m(Sub, "face_cloud", 1.0), 0.100),
                    ),
                    // A stroke reads polished where the brush loaded the face:
                    // in the reference the two maps agree about where the
                    // brushing went, and its stroke patches are firm-edged and
                    // clearly darker. The same two stacked planes the colour
                    // uses, so the roughness carries the same hard-rimmed
                    // facets and not a second, softer set of its own.
                    m(
                        Sub,
                        m(Mul, "stroke_grain", 0.07),
                        m(
                            Add,
                            m(Mul, "stroke_lo", 0.038),
                            m(Mul, "stroke_firm", 0.050),
                        ),
                    ),
                ),
                m(
                    Sub,
                    m(
                        Mul,
                        // The flake lattice through every sliver, for the
                        // reason the dark ticks are broken up by the grain:
                        // an unmodulated scatter of identical capsules
                        // reads as printed rice, not as a chipped face.
                        m(Mul, "shards", remap("cell_r", 0.15, 1.25)),
                        m(Add, 0.08, m(Mul, "fleck_patch", 0.26)),
                    ),
                    m(Mul, "inclusions", 0.075),
                ),
            ),
        ),
    )
    // 1294951462 Copy(stone_id, white, opacity $Roughness_TopVariation), which
    // is what gives the reference its 0.442..0.564 spread of per-stone means.
    .node("per_stone_r", remap("stone_id", 0.5, 1.0))
    // The reference's roughness carries more power at a stone's own scale than
    // at any finer one — 0.037 against 0.018 in the octave bands — so the
    // per-stone number is worth a level of its own and not only a scale on the
    // shine it is multiplied into below.
    .node("r_stone_step", m(Mul, m(Sub, "per_stone_r", 0.75), 0.40))
    // 1294860459 subtract the broad shine, $Roughness_TopShine — off the same
    // smudge the colour's shine reads, so a clean patch is a bright and a
    // polished one. Off the occlusion's high pass, as it was, this term was a
    // function of the distance to the joint: full in the middle of a face and
    // zero at every toe, which left the flat light ribbon the review found
    // around every stone and the hard step where it ran out.
    .node(
        "r_shine",
        m(
            Sub,
            m(Add, "stone_rough_base", "r_stone_step"),
            m(
                Mul,
                m(
                    Mul,
                    "per_stone_r",
                    Levels::new("smudge").in_range(0.46, 0.96),
                ),
                0.09,
            ),
        ),
    )
    // 1294860946 $Roughness_Edge, and what the reference's face really does
    // near an edge: it gets gradually, irregularly rougher over fifteen to
    // thirty pixels — measured 0.486 deep inside, 0.493 at twenty, 0.527 at
    // ten, 0.597 at two — with one thin bright line right at the rim. A wide
    // range at a gamma is that profile: full at the toe, half of it nine pixels
    // in, an eighth by twenty, and with zero slope where it runs out, so there
    // is nothing to draw an inner contour. The line the old `arris` drew sat at
    // signed 0.70, which the profile curve puts nineteen pixels inside the
    // face, and it was the hard boundary on the inside of that ribbon.
    .node(
        "r_toe_noise",
        Noise::perlin()
            .period(32)
            .octaves(3)
            .persistence(0.6)
            .seed(seed.wrapping_add(47)),
    )
    .node(
        "r_lift",
        m(
            Mul,
            Levels::new("signed_mod").in_range(0.86, 0.55).gamma(2.4),
            remap("r_toe_noise", 0.35, 1.25),
        ),
    )
    // Two pixels wide, hard against the height crossing at signed 0.30 rather
    // than up at the arris.
    .node(
        "r_rim_line",
        m(
            Mul,
            Levels::new("signed_mod").in_range(0.548, 0.566),
            Levels::new("signed_mod").in_range(0.594, 0.572),
        ),
    )
    .node(
        "r_edge",
        m(
            Add,
            m(Sub, "r_shine", m(Mul, "edge_peak", 0.02)),
            m(Add, m(Mul, "r_lift", 0.155), m(Mul, "r_rim_line", 0.115)),
        ),
    )
    // 1294861041 add the dirt, $Roughness_Dirt.
    // Half what it was. The stain hugs the toe, so a fifth of a roughness unit
    // through it drew a soft light ribbon ten to twenty pixels wide around every
    // stone — the same halo the ownership fix had just removed, arriving by
    // another road. The gradual roughening toward an edge belongs to `r_toe`,
    // which is irregular and does not follow the silhouette.
    .node(
        "stone_rough",
        Clamp::new(m(Add, "r_edge", m(Mul, "dirt_mask", 0.10))),
    )
    // 1294861187 histogram_range(.3, $Roughness_GroutBase), then 1294861196,
    // which takes off the aggregate so the chips read smoother than the bed they
    // sit in. Measured on the reference the sand runs 0.703 and a chip 0.574, so
    // the step is 0.13 and it is taken through the same hard-edged mask the
    // colour uses: a flat smooth disc, not a soft dimple. The bed's own speckle
    // is worth more than it was — the reference sand's five-pixel high pass is
    // 0.081 against 0.067 here — and it is deliberately not the colour's grain,
    // because in the reference the two are uncorrelated.
    .node(
        "grout_rough",
        Clamp::new(m(
            Sub,
            m(
                Add,
                m(
                    Add,
                    m(Sub, p("roughness"), 0.052),
                    m(Mul, "rough_field", 0.22),
                ),
                m(
                    Add,
                    m(Mul, m(Sub, "speck_sum", "speck_lp"), 0.09),
                    // A grain of sand is rough or smooth by which grain it is,
                    // and in the reference that has nothing to do with how
                    // bright it is: the correlation between the joint's
                    // roughness and its colour is -0.09. So the bed's per-texel
                    // roughness is the middle band of the same hash rather than
                    // the same cut of it the colour takes, which straddles the
                    // colour's threshold and comes out all but uncorrelated
                    // with it while costing no field of its own.
                    m(Mul, m(Sub, "sand_dither_band", 0.45), 0.13),
                ),
            ),
            m(Mul, "chip_mask", 0.13),
        )),
    )
    // 1294861282: ownership, but the crisp one. The colour's `own_color` carries
    // the sand bleed, which is a colour idea — grains climbing the skirt — and
    // handing a third of the joint's much brighter roughness to a twenty-five
    // pixel band of every chamfer is what drew the flat light blob still visible
    // around each stone in the last roughness crop. Roughness changes hands at
    // the height crossing and nowhere else.
    .node("rough_mix", Mix::new("grout_rough", "stone_rough", "own"))
    // All the reference keeps of a rim in roughness is a thin rough line right
    // at the toe, where the sand packs against the stone. The band that used to
    // stand here was the joint's own roughness arriving through the eaten
    // ownership mask, twenty pixels of it, flat and light.
    .node(
        "rough_rim",
        m(Add, "rough_mix", m(Mul, "contact_line", 0.10)),
    )
    // 1294898694, sharpen at $Roughness_Sharpness 1.0.
    .node(
        "rough",
        Clamp::new(m(
            Add,
            "rough_rim",
            m(
                Mul,
                m(Sub, "rough_rim", Blur::new("rough_rim").radius(0.0011)),
                0.3,
            ),
        )),
    );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("rough")
            .metallic(0.0)
            .occlusion("ao")
            .extra("base", "rounded_mask")
            .extra("damage", "damaged_mask")
            .extra("sloping", "sloped")
            .extra("surface", "stone_height")
            .extra("grout", "grout")
            .extra("stone_mask", "stone_mask")
            .extra("stone_id", "stone_id")
            .extra("dirt", "dirt_mask")
            .extra("own", "own"),
        0.025,
    )
}

/// A number per texel in `0..=1`, in eight steps, on the dissolve hash.
///
/// The hash a [`BlendMode::Dissolve`] reads is the one field in the crate finer
/// than the 512-texel lattice a bake has to resolve, and it is the only way to
/// author one- and two-pixel grain at this resolution. A single dissolve is
/// two-tone, which is what made the last pass's face read as a halftone print.
/// Every dissolve hashes the same seed at the same coordinate, though, so seven
/// thresholds on it count how far down the distribution the texel fell: that
/// count *is* the hash, quantised to eight levels, which at the amplitudes a
/// grit is used at is a continuous tone.
fn dither_level() -> Math {
    let at = |threshold: f32| Blend::new(BlendMode::Dissolve, 0.0, 1.0).opacity(threshold);
    let sum = m(
        Add,
        m(Add, at(0.125), m(Add, at(0.250), at(0.375))),
        m(
            Add,
            m(Add, at(0.500), at(0.625)),
            m(Add, at(0.750), at(0.875)),
        ),
    );
    m(Mul, sum, 1.0 / 7.0)
}

/// One graded layer of the grout bed: flat-topped discs on a jittered lattice.
///
/// `period` is the lattice, `lo` and `hi` bound the radius in cell units, `gate`
/// is the share of sites dropped, and `amp` is the layer's height. The profile
/// is a spherical cap of the distance to the site, so a level set is a circle
/// and nothing about the cell's polygon reaches the height. Keeping the radius a
/// small fraction of the cell is what guarantees that: the dome finishes before
/// the cell boundary, so the boundary — where the distance field switches cones
/// and the per-cell radius steps — only ever falls where the dome is zero.
///
/// The cap is then cut off at a per-site plateau. Read back, the reference's
/// aggregate is not a bed of beads: it is flat flakes of stone lying in the
/// sand, a crisp rim and a level top. A `Min` against a constant per cell is the
/// whole of that, and because it only ever lowers the cap it cannot push a disc
/// past the cell boundary the rule above keeps it inside. The plateau is taken
/// off the same hash the gate reads, so the sites that survive get the taller
/// tops and one Voronoi read serves both.
fn stone_dome(seed: u32, period: u32, lo: f32, hi: f32, gate: f32, amp: f32) -> Math {
    let cell = || {
        Voronoi::new()
            .period(period)
            .seed(seed)
            .output(VoronoiOutput::Cell)
    };
    let hash = || Math::unary(Fract, m(Mul, cell(), 13.0));
    let reach = || {
        Clamp::new(m(
            Div,
            Voronoi::new()
                .period(period)
                .seed(seed)
                .output(VoronoiOutput::Distance),
            remap(cell(), lo, hi),
        ))
    };
    m(
        Mul,
        m(
            Min,
            Levels::new(m(Sub, 1.0, m(Mul, reach(), reach())))
                .gamma(0.7)
                .out_range(0.0, amp),
            m(Mul, amp, remap(hash(), 0.26, 0.74)),
        ),
        m(Step, hash(), gate),
    )
}
