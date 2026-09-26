//! Dense clipped turf, matched to the supplied `BlenderKit` studio sphere.
//!
//! The reference is almost entirely crown: short overlapping blades, bright
//! yellow-green tips and dark green recesses, with no exposed soil.
//!
//! # Three scatters, two halves
//!
//! The graph carries three strand layers and one PBR output, and they are the
//! *same* strands.  `blades` scatters one blade per cell of a 256-cell lattice
//! over the two-metre repeat; `fibres` scatters four much finer, shorter ones
//! per cell of the same lattice, and it is what makes the mat read as fibre
//! rather than as a scatter of leaves; `stragglers` scatters a few upright
//! escapes on half that lattice, and it is what gives the mat an edge.  All
//! three read their fields — where the pile is thick, how long it grows, which
//! way it lies — off the ordinary noise nodes the surface colour is built
//! from.  Geometry is grown from those three scatters.  So is the relief:
//! [`StrandRelief`] splats the same strands, seen from directly above, back
//! into planes the PBR half reads.
//!
//! That is the whole of this pass, and it replaced six `Tile` beds of capsules
//! that only *suggested* blades.  The difference is not that the drawing is
//! better; it is that there is now one drawing.  A blade the camera sees
//! standing up close is the same blade the texture shades when the geometry has
//! faded out, because both asked the same scatter where it was.
//!
//! # Which way round the dark goes
//!
//! A lawn is a canopy, and the ground under a canopy is the darkest thing in
//! frame.  The crown is the brightest.  So the surface here is built by mixing
//! *up* from a dark bed towards the strand colours by how much strand covers
//! the texel, and then shading the result by how high the canopy stands over
//! it: a texel with nothing on it is bare ground under 40 mm of grass and is
//! shaded like it, and a texel under a tip is lit like a tip.
//!
//! The pass before this one had it the other way round — it started from the
//! full-brightness bed and *darkened* wherever a strand stood, which is the one
//! arrangement that cannot be right, because the thing being darkened is the
//! thing that is in the light.  The close-up it produced was a pale floor with
//! black scratches on it.
//!
//! # What may read what
//!
//! A strand field may not reach a relief — it would be asking for itself, and
//! the lowering refuses it by path.  So the graph is in two sections and the
//! order matters: everything above `relief_cover` is the **bed**, built out of
//! noise alone and readable by either half, and everything from `relief_cover`
//! down is the surface, which reads the blades.  The strand layer's own colour
//! and roughness therefore come off the bed rather than off the finished
//! surface — which is also the honest reading, since a blade is lit by the
//! studio rather than by the texture under it.
//!
//! The pile is [`HEIGHT_SCALE_METRES`] tall, and every layer's
//! `length_metres` is that number or a fraction of it, so
//! [`StrandReliefOutput::Height`] — metres above the surface in units of the
//! layer's own length — lands directly on the height map's `0..=1` and the
//! three layers' heights are in the same units as each other.  At two segments
//! a blade is four triangles and at one a fibre is two, so a full-detail
//! repeat is **688 818 triangles** across the three layers — 62 251 blades,
//! 204 799 fibres and 7 554 stragglers — against a budget of 800 000; a
//! surface that cannot afford that thins it with
//! [`ashlar_surface::StrandSettings::density`], which keeps a prefix rather than
//! reshuffling the lawn.
#![allow(clippy::too_many_lines, reason = "authored reference recipe")]
use super::reference_support::{TILE_METRES, finish, m, p, remap};
use crate::{
    Channel, MaterialGraph,
    MathOp::{Add, Max, Mul, Sub},
    Param, PbrOutput, StrandLayer,
    nodes::{
        Clamp, Colorize, Combine2, Decompose, Direction, Invert, Levels, Mix, Noise, StrandRelief,
        StrandReliefOutput, Voronoi, VoronoiOutput,
    },
};

/// Physical height range of the pile, and the length of its longest blade.
///
/// Forty millimetres, read off the reference's own silhouette rather than
/// assumed.  The studio sphere is 1.273 m across — `Sphere::new(2.0 / PI)` is a
/// radius — and its four-metre circumference carries two repeats, so a
/// millimetre on that ball is a millimetre of this material.  Reading the
/// reference's rim on those terms: the band of partial coverage where the
/// strands thin into the background is 10.1 mm deep at the median and 22.0 mm
/// at the ninetieth percentile, and the radius, with the sphere itself
/// subtracted, runs from −8.8 to +10.0 mm on a 23 mm cycle.  A blade leaning as
/// far as this one does stands about half its length proud, so a crown whose
/// ninetieth percentile is 22 mm is a blade of roughly forty.
///
/// What that assumes, and it cannot be checked, is that the reference sphere
/// shows two repeats round as ours does.  Nothing was supplied with it that
/// says so.  Read it as: at the tiling that makes the two pictures the same
/// picture, the grass is 40 mm.
///
/// The 18 mm this was is what the first pass declared without measuring, and it
/// is the single number most of the flatness came from: at 18 mm, with the same
/// lean, the same measurement of our own sphere gives a 3.7 mm median band and
/// nothing at all past 10 mm, which is the even fine fuzz the render had
/// instead of a lumpy crown.
pub const HEIGHT_SCALE_METRES: f32 = 0.040;

/// Blades across the two-metre repeat, on each axis.
///
/// A power of two, and the plan's own number: 65 536 strands over four square
/// metres, about sixteen thousand a square metre, which is a quarter of a real
/// lawn and as much as anything is going to draw.
pub const BLADE_COUNT: u32 = 256;

/// Segments per blade.
///
/// Two rather than three.  A third segment buys a curve that is a pixel deep
/// at any distance a blade is still geometry, and it costs a third of the whole
/// layer's triangles — which is the straggler layer twice over.
pub const BLADE_SEGMENTS: u32 = 2;

/// Blades standing in each cell of the lattice.
///
/// One.  The mat's body is the `fibres` layer below, which stands four to a
/// cell at half the triangles each; this layer is the *coarse* half of the pair
/// and wants its triangles spent on two segments of curve rather than on more
/// leaves.  With the stragglers the three come to 688 818 triangles over one
/// full-detail repeat, inside the 800 000 this material is allowed.
pub const BLADES_PER_CELL: u32 = 1;

/// Fibres standing in each cell of the same lattice.
///
/// Four, which is 204 799 fibres over four square metres once the density
/// threshold has taken its share — 51 000 a square metre, and three and a
/// quarter times the blade count.  It is `per_cell` rather than a finer `count` because
/// `count` is a lattice the material's repeat has to carry and this is not: the
/// strands of a cell share its coordinate and differ only in the salt their
/// hashes are taken under.  Sharing the lattice with `blades` is also what
/// keeps the material's period one number rather than two.
///
/// The fourth one is what the triangle budget buys instead of a second blade
/// per cell.  A fibre is two triangles and a blade is four, and what the
/// reference's ground needs is more mat rather than more leaves.
pub const FIBRES_PER_CELL: u32 = 4;

/// How long a fibre is, in metres.
///
/// Seventeen millimetres, a little over two fifths of the pile.  The reference
/// is not a field of leaves; it is a mat whose surface is thousands of fine
/// ends, with the occasional longer blade over them.  A layer that is half as
/// long and four times as many is what draws that, and it costs a third of what
/// another layer of full-length blades would.
pub const FIBRE_LENGTH_METRES: f32 = HEIGHT_SCALE_METRES * 0.42;

/// How wide a fibre is at its root, in metres.
///
/// 1.7 mm, which is a little under two texels of the shipped 2048² maps and
/// about four fifths of a blade.  It is held above the one-texel line
/// deliberately: under it the splat can no longer draw the fibre and the bake
/// says so by path, so a relief of a layer thinner than that would be a lattice
/// warning and a plane of dots.  What makes a fibre read as finer than a blade
/// is therefore the width *and* the length together, not the width alone.
pub const FIBRE_WIDTH_METRES: f32 = 0.0017;

/// Segments per fibre.
///
/// One, so a fibre is a single tapered quad: two triangles. At nine
/// millimetres and a grazing angle there is no curve in it to see, and the
/// triangles saved are what pay for there being three of them per cell.
pub const FIBRE_SEGMENTS: u32 = 1;

/// Stragglers across the repeat, on each axis.
///
/// Half the blade lattice, so it divides it and the material's period stays
/// 256. One per cell of that, and the density threshold keeps about a third of
/// them: 7 554 escapes over four square metres, which is 1 900 a square metre
/// against the blades' 15 600. They are meant to be countable.
pub const ESCAPE_COUNT: u32 = 128;

/// How wide a straggler is at its root, in metres.
///
/// 1.8 mm, between a blade and a fibre. It is the length that makes it read as
/// an escape, not the width, and a narrower one would disappear at the rim,
/// which is the only place it is doing any work.
pub const ESCAPE_WIDTH_METRES: f32 = 0.0018;

/// Clump cells across the repeat, on each axis.
///
/// Sixty-four over two metres is a tuft every 31 mm, which is sixteen blades
/// and sixty-four fibres to a clump at this lattice.  The tufts are what the
/// reference is made of: it is not a comb of blades all leaning one way, it is
/// a mat of little bunches that each agree with themselves and disagree with
/// their neighbours.
///
/// Thirty-one millimetres is also the number the reference's silhouette gives
/// back.  Detrend its radius against the sphere and the residual crosses zero
/// 240 times a turn, which is a 33 mm cycle on a 1.99 m circumference: the
/// lumps in that rim are tufts, and this lattice is the right size for them
/// already.  What was missing was not a finer lattice but tufts that differ
/// from each other, which is [`StrandLayer::clumps`]' share rather than its
/// count.
pub const CLUMP_COUNT: u32 = 64;

/// How wide a blade is at its root, in metres.
///
/// 2.2 mm, which at the shipped 2048² maps is a little over two texels of the
/// relief — well above the one-texel line the bake warns at, and against a
/// 40 mm length a 1:18 ribbon, which is finer in proportion than the 1:14 the
/// old 1.3 mm blade was at 18 mm.  A blade earns the extra millimetre twice
/// over: it is the cheapest coverage in the material, because width costs no
/// triangles at all, and the reference's strokes are coarser than ours were
/// rather than finer.  Under one texel the splat can no longer draw it and the
/// bake says so by path; the 512² diagnostics are deliberately below that line,
/// and the warning they print is the check working rather than failing.
pub const BLADE_WIDTH_METRES: f32 = 0.0022;

/// A full-coverage, short turf surface over a two-metre repeat.
pub fn grass(seed: u32) -> MaterialGraph {
    let g = MaterialGraph::builder("library:grass")
        .param(Param::float("lushness", 0.74).range(0.0, 1.0))
        .param(Param::float("tip_brightness", 0.66).range(0.0, 1.0))
        .param(Param::float("roughness", 0.91).range(0.65, 1.0))
        // How much of the lawn has gone to thatch, in `0..=1` of the area.
        //
        // It earns a parameter of its own rather than folding into `lushness`
        // because the two are different statements.  `lushness` says what
        // colour the whole lawn is; `dryness` says how much of it disagrees
        // with that colour, and where.  A lawn that is uniformly olive and a
        // lawn that is green with tan patches through it are the same mean
        // colour and do not read alike at any distance.
        .param(Param::float("dryness", 0.30).range(0.0, 1.0))
        // --- the bed: noise alone, and the half a strand field may read. ---
        // Slow patches keep the two-metre repeat from becoming one flat green.
        .node(
            "patches",
            Noise::perlin()
                .period(4)
                .octaves(4)
                .persistence(0.56)
                .seed(seed.wrapping_add(11)),
        )
        // The scale the reference varies over hardest, and the one this
        // material had no field at: a twelve-centimetre cell, so a sphere shows
        // ten or so of them.  `patches` is a half-metre and `clumps` is a tuft;
        // between them sat the hand-sized patch of tan thatch, the hand-sized
        // patch of bright yellow-olive and the hand-sized patch of darker green
        // that the reference is mostly made of.
        .node(
            "mid",
            Noise::perlin()
                .period(16)
                .octaves(3)
                .persistence(0.55)
                .seed(seed.wrapping_add(37)),
        )
        .node(
            "clumps",
            Noise::perlin()
                .period(32)
                .octaves(4)
                .persistence(0.62)
                .seed(seed.wrapping_add(23)),
        )
        // The tuft band, and it needs a generator of its own.  Band-pass the
        // reference's sphere and ours between a ten and a thirty millimetre
        // blur and the reference deviates by 0.31 of its mean where we managed
        // 0.20, and no amount of gain on the fields above closes it: a fractal
        // noise puts most of its amplitude in its *base* octave, so `clumps` at
        // a 62 mm cell and `mid` at 125 mm both answer in the band above this
        // one.  Two octaves off a 31 mm cell answer in it, and 31 mm is the
        // clump lattice, which is the point — a tuft that is brighter than its
        // neighbour should also be the tuft that grew thicker.
        .node(
            "tufts",
            Noise::perlin()
                .period(64)
                .octaves(2)
                .persistence(0.60)
                .seed(seed.wrapping_add(67)),
        )
        .node(
            "micro",
            Noise::perlin()
                .period(128)
                .octaves(3)
                .persistence(0.58)
                .seed(seed.wrapping_add(53)),
        )
        // The green the pile is growing out of, at the four scales the
        // reference varies over: metre-wide patches, the hand-sized variation
        // above, clump-sized bunching and a fine break-up.  It is the tone both
        // halves colour from, which is what keeps a blade the same green as the
        // recess it stands in.
        .node(
            "bed_mix",
            Clamp::new(m(
                Add,
                0.06,
                m(
                    Add,
                    m(Add, m(Mul, "patches", 0.18), m(Mul, "mid", 0.22)),
                    m(
                        Add,
                        m(Add, m(Mul, "clumps", 0.22), m(Mul, "tufts", 0.30)),
                        m(Mul, "micro", 0.10),
                    ),
                ),
            )),
        )
        // And stretched, which is the correction this pass owes the one before
        // it.  Four noises averaged together do not make a wider field than one
        // of them; they make a *narrower* one, because their deviations cancel.
        // Measured off the 512² stage, the sum above runs 0.46 to 0.60 between
        // its fifth and ninety-fifth percentiles, so every colour ramp below it
        // was being read through a seventh of its own range and the whole lawn
        // came out one tone.  Blur our sphere and the reference's until the
        // strands disappear and the numbers say the same thing: at a four
        // centimetre blur the reference's luminance deviates by 0.062 and ours,
        // before this node, by 0.020.
        //
        // The gain is four, which puts that same fifth-to-ninety-fifth band
        // across most of the ramp and leaves the tails to clamp.
        .node(
            "bed_tone",
            Levels::new("bed_mix")
                .in_range(0.48, 0.66)
                .out_range(0.10, 1.0),
        )
        .node(
            "grass_color",
            Colorize::new("bed_tone").gradient([
                (0.0, [0.028, 0.044, 0.006]),
                (0.34, [0.100, 0.150, 0.018]),
                (0.66, [0.266, 0.360, 0.044]),
                (1.0, [0.560, 0.710, 0.089]),
            ]),
        )
        // Thatch, and it is tan rather than olive.  The old dry ramp was a
        // yellower green, so mixing towards it only desaturated the lawn; the
        // reference's dead patches are browner than its live grass in hue as
        // well as darker in the green channel.
        .node(
            "dry_color",
            Colorize::new("bed_tone").gradient([
                (0.0, [0.050, 0.046, 0.011]),
                (0.50, [0.222, 0.194, 0.050]),
                (1.0, [0.572, 0.500, 0.135]),
            ]),
        )
        // Where the thatch is.  A contrast-stretched hand-and-arm-scale field,
        // pushed either side of zero by `dryness`, so the parameter moves the
        // *area* that has dried rather than the colour of the whole lawn.
        .node(
            "dry_shape",
            Levels::new(m(Add, m(Mul, "mid", 0.60), m(Mul, "patches", 0.40))).in_range(0.34, 0.72),
        )
        .node(
            "dry_mask",
            Clamp::new(m(
                Add,
                m(Mul, "dry_shape", 1.7),
                m(Sub, m(Mul, p("dryness"), 1.9), 1.0),
            )),
        )
        // Lushness shifts the bed from olive/dry toward the saturated crown in
        // the reference while tip brightness remains a separate studio-match
        // control.  The dry mask subtracts from it locally, which is what puts
        // tan patches in a green lawn instead of an evenly olive one.
        .node(
            "lush_field",
            Clamp::new(m(Sub, p("lushness"), m(Mul, "dry_mask", 0.85))),
        )
        .node(
            "lush_color",
            Mix::new("dry_color", "grass_color", "lush_field"),
        )
        // The bright end every tip in the material is mixed towards, and it is
        // a ramp off `bed_tone` rather than the one constant it used to be.
        //
        // That constant was quietly the largest flattener in the graph. A tip
        // is the brightest and most visible part of a blade, it was two thirds
        // of a fixed colour, and the relief then averages the tips of every
        // strand crossing a texel — so two thirds of the lawn's crown carried
        // no spatial variation at all, however wide the bed's own range was.
        // Running the crown off the same field the bed runs off is what lets a
        // patch of bright yellow-olive and a patch of darker green be the same
        // material.
        .node(
            "crown_ramp",
            Colorize::new("bed_tone").gradient([
                (0.0, [0.128, 0.172, 0.022]),
                (0.45, [0.428, 0.540, 0.063]),
                (1.0, [0.880, 0.990, 0.140]),
            ]),
        )
        // The roughness the blades carry and the surface starts from.  A blade
        // is matte with a small highlight; the tip mask below takes a little
        // more off where the crown catches the light, and that part is the
        // surface's alone because it reads the relief.
        .node(
            "bed_rough",
            Clamp::new(m(Add, p("roughness"), remap("micro", -0.035, 0.035))).range(0.70, 1.0),
        );

    finish(
        surface(stragglers(fibres(blades(g, seed), seed), seed)),
        PbrOutput::new()
            .base_color("color")
            .roughness("rough")
            .metallic(0.0)
            .occlusion("ao")
            .extra("patches", "patches")
            .extra("mid", "mid")
            .extra("clumps", "clumps")
            .extra("bed_tone", "bed_tone")
            .extra("dry_mask", "dry_mask")
            .extra("relief_cover", "relief_cover")
            .extra("fibre_cover", "fibre_cover")
            .extra("straggler_cover", "straggler_cover")
            .extra("relief_height", "relief_height")
            .extra("pile", "pile")
            .extra("mat", "mat")
            .extra("canopy", "canopy")
            .extra("tip_mask", "tip_mask"),
        HEIGHT_SCALE_METRES,
    )
}

/// The `blades` strand layer, over the bed the section above built.
///
/// Every field below reads a node that is already in the graph, which is what
/// keeps the geometry and the texture one surface: `patches` decides where the
/// pile is thick in both, `clumps` decides where it is long in both, and the
/// colours are the same `lush_color` the base colour is mixed out of.  The only
/// nodes added here are the three a strand needs and a texel does not — an
/// orientation, a gathering offset, and a darker root colour.
fn blades(g: crate::MaterialGraphBuilder, seed: u32) -> crate::MaterialGraphBuilder {
    g
        // Clipped turf combs rather than fans: the blades of a patch lie the
        // same way, and the way they lie follows the contours of a field rather
        // than pointing up its slope.  That quarter turn is the whole
        // difference between grass and fur standing on end.
        //
        // It reads `mid` rather than `clumps`, so the flow swirls over a
        // hand's width and not over a tuft.  What turns one tuft against the
        // next is the clump's own hash, through `direction_variation` shared by
        // `clump_share` below; a flow field fine enough to do that job itself
        // would give every tuft the same *kind* of turn as its neighbour.
        .node(
            "blade_flow",
            Direction::from_slope("clumps").rotate_quarter(),
        )
        // A tuft is several roots gathered onto one point.  The Voronoi offset
        // is the vector all the way to the cell point, which would stack a
        // whole cell of blades on top of each other, so it is taken apart and
        // scaled: a third of the way in bunches them and leaves them apart.
        // `Combine2` is here because nothing in the vocabulary scales a `Vec2`.
        .node(
            "tuft_offset",
            Voronoi::new()
                .period(128)
                .jitter(0.85)
                .seed(seed.wrapping_add(71))
                .output(VoronoiOutput::Offset),
        )
        .node(
            "tuft_u",
            m(Mul, Decompose::new("tuft_offset", Channel::R), 0.34),
        )
        .node(
            "tuft_v",
            m(Mul, Decompose::new("tuft_offset", Channel::G), 0.34),
        )
        .node("tuft", Combine2::new("tuft_u", "tuft_v"))
        // Density is a keep threshold, so this is the fraction of the lattice
        // that carries a blade at all.  It never falls below four fifths: the
        // reference has no exposed soil, and the relief is now the only thing
        // drawing the crown, so a cell that carries no blade is a hole in the
        // texture as well as in the geometry.
        .node(
            "blade_cover",
            remap(
                m(Add, m(Mul, "patches", 0.45), m(Mul, "clumps", 0.55)),
                0.90,
                1.0,
            ),
        )
        // How long a blade grows before its own hash and its tuft's take some
        // of it away.  The floor is low, and it is low on purpose: with
        // `length_variation` mostly shared by the clump, the range from here
        // down is what makes one tuft a stubble and the tuft beside it a
        // straggler, which is the lumpy crown the reference's rim measures.
        .node(
            "blade_length",
            remap(
                m(
                    Add,
                    m(Add, m(Mul, "clumps", 0.30), m(Mul, "tufts", 0.34)),
                    m(Add, m(Mul, "mid", 0.20), m(Mul, "patches", 0.16)),
                ),
                0.46,
                1.0,
            ),
        )
        // A clipped lawn leans a little and everywhere; the recesses lie over
        // further than the crowns, which is what `patches` is reading here.
        // The lean is also what gives a blade a footprint at all — a strand
        // standing straight up casts a disc the width of itself — so a lawn
        // whose blades never lay over would have a relief of dots.
        // A clipped lawn lies over a long way: a blade standing straight up
        // casts a footprint the width of itself, so an upright layer splats
        // dots rather than strokes, and the reference is all strokes.
        .node("blade_lean", remap("patches", 0.32, 0.60))
        // Held back from the point where `lean + bend` carries the tip past
        // horizontal and the blade curls into the ground: the splat floors a
        // negative height at the surface, but a blade that spends half its
        // length under the bed is length nobody sees.
        .node("blade_bend", remap("clumps", 0.26, 0.54))
        // The root is the colour of the pile it grows out of, and no darker.
        // A blade drawn darker than its bed is the exact inverse of the
        // reference, where the crown is the brightest thing in frame and the
        // dark is the gap between blades; the relief's own occlusion is what
        // supplies that dark now.
        .node("blade_root_color", m(Mul, "lush_color", 0.92))
        // The tip is the brightest thing the material draws, which is what the
        // reference's crown is.  Warmer than it was: measured on the same
        // window of both spheres, the reference's mean linear red is nine
        // tenths of its green and ours was three quarters, so every ramp in
        // this graph moves the same way.
        .node(
            "blade_tip_color",
            Mix::new("lush_color", "crown_ramp", p("tip_brightness")),
        )
        .strands(
            "blades",
            StrandLayer::new()
                .count(BLADE_COUNT)
                .seed(seed.wrapping_add(97))
                .density("blade_cover")
                .length("blade_length")
                .lean("blade_lean")
                .bend("blade_bend")
                .direction("blade_flow")
                .clump("tuft")
                .colors("blade_root_color", "blade_tip_color")
                .roughness("bed_rough")
                .metres(HEIGHT_SCALE_METRES, BLADE_WIDTH_METRES)
                // One blade to a cell, gathered into tufts of about sixteen
                // that share most of their variation and lean their tips
                // together.
                .per_cell(BLADES_PER_CELL)
                // A share of 0.88, where it was 0.65.  That number is what
                // decides whether a lawn is tufted or speckled, and it decides
                // it twice: it is the fraction of *every* variation that comes
                // off the tuft's hash rather than the blade's, so at 0.88 the
                // sixteen blades of a tuft agree about which way they lie to
                // within a dozen degrees and about how long they are to within
                // a twentieth, while the tuft beside them disagrees on both by
                // most of the range.  At 0.65 they disagreed with each other
                // nearly as much as with their neighbours, and sixteen blades
                // disagreeing inside three centimetres is a speckle.
                .clumps(CLUMP_COUNT, 0.92, 0.62)
                .clump_tint(0.42)
                // A blade is widest a third of the way up, so the bright end of
                // its colour ramp has area.  At a midpoint of zero a fully
                // tapered blade is a triangle whose tip is one texel wide, and
                // the splat is then weighted almost entirely towards the dark
                // root.
                .midpoint(0.30)
                // So that a patch of blades leaning alike is not also presented
                // alike: a half turn either way puts edge-on blades among the
                // flat-on ones.
                .facing_variation(0.85)
                // A seventh of the pile's height of burial, which is what gives
                // the mat its short, half-emerged blades without shortening the
                // distribution they came from.
                .height_offset(HEIGHT_SCALE_METRES * 0.11)
                // The spatial fields above vary a blade with its *neighbours*;
                // these vary it against them, which is what keeps a lattice of
                // 65 536 cells from reading as a lattice.  A variation only
                // ever takes length away, so the half of it that is left after
                // `clump_share` is also what makes 28 mm the straggler rather
                // than the rule.
                .length_variation(0.55)
                .width_variation(0.35)
                // Sixty-two hundredths of a half turn, where it was eighty.
                // Almost all of it now goes to the clump, so what this number
                // sets is how far one *tuft* turns from the next: a hundred
                // degrees either side of a flow that drifts over a hand's
                // width, which is the swirl the reference has between
                // neighbouring tufts.  Inside a tuft the same number leaves
                // thirteen degrees, which is a comb.
                .direction_variation(0.62)
                .lean_variation(0.48)
                // The clump owns how far a blade curls as well as how long and
                // which way it lies, which is what the Tsushima accounts put
                // under it: one tuft curls together and the tuft beside it
                // curls differently.
                .bend_variation(0.45)
                .segments(BLADE_SEGMENTS)
                .taper(0.90)
                // Small, where it used to be nearly half.  The dark in a lawn
                // belongs *between* the blades and not on them: darkening the
                // blade itself puts the shadow on the brightest thing in frame,
                // which is what made the close-up read as dark gouges on a pale
                // floor.  `relief_shade` below is where the dark went instead.
                .root_occlusion(0.16),
        )
}

/// The `fibres` layer: the fine half of the mat.
///
/// The gap the reference kept showing up was one of *scale*, not of shape. A
/// blade 1.8 mm wide and 18 mm long is a 1:10 ribbon, and at arm's length a
/// scatter of those reads as a scatter of little leaves lying on a floor. The
/// reference's surface is made of far finer ends, thousands of them, with a few
/// blades over the top — which is the layering every Substance grass breakdown
/// does and which this material had no equivalent for.
///
/// So this layer is short, narrow, four to a cell and one segment each. It
/// reads the same `patches` and `clumps` fields the blades do, so the two
/// agree about where the pile is thick, and it lies over further: a fibre that
/// stood up would cast a dot where the point of this layer is to cast a mat.
fn fibres(g: crate::MaterialGraphBuilder, seed: u32) -> crate::MaterialGraphBuilder {
    g
        // Nearly everywhere, and thinner only where the bed itself is thin.
        // This is the layer that fills the ground in: a hole in it is a hole in
        // the mat and a hole in the texture under it.
        .node(
            "fuzz_cover",
            remap(
                m(
                    Add,
                    m(Mul, "patches", 0.20),
                    m(
                        Add,
                        m(Mul, "mid", 0.18),
                        m(Add, m(Mul, "clumps", 0.34), m(Mul, "tufts", 0.28)),
                    ),
                ),
                0.62,
                0.94,
            ),
        )
        .node(
            "fuzz_length",
            remap(
                m(
                    Add,
                    m(Mul, "clumps", 0.42),
                    m(Add, m(Mul, "mid", 0.30), m(Mul, "micro", 0.28)),
                ),
                0.46,
                1.0,
            ),
        )
        // Further over than a blade. A fibre thirteen millimetres long standing
        // upright covers about one texel of the relief; the same fibre laid
        // over at sixty degrees covers twelve, and twelve is a mat.
        .node("fuzz_lean", remap("micro", 0.54, 0.82))
        .node("fuzz_bend", remap("patches", 0.18, 0.44))
        // Darker at the root than a blade is: a fibre sits *in* the mat rather
        // than over it, so most of its length is in the pile's own shade.
        .node("fuzz_root_color", m(Mul, "lush_color", 0.84))
        .node(
            "fuzz_tip_color",
            Mix::new(
                "lush_color",
                m(Mul, "crown_ramp", 0.80),
                p("tip_brightness"),
            ),
        )
        .strands(
            "fibres",
            StrandLayer::new()
                .count(BLADE_COUNT)
                .seed(seed.wrapping_add(151))
                .density("fuzz_cover")
                .length("fuzz_length")
                .lean("fuzz_lean")
                .bend("fuzz_bend")
                .direction("blade_flow")
                .clump("tuft")
                .colors("fuzz_root_color", "fuzz_tip_color")
                .roughness("bed_rough")
                .metres(FIBRE_LENGTH_METRES, FIBRE_WIDTH_METRES)
                .per_cell(FIBRES_PER_CELL)
                // The same clump lattice the blades use, so a tuft is one tuft
                // rather than two that happen to overlap, but sharing less of
                // it: a bunch of fine ends splays where a bunch of leaves
                // agrees.  Still well above the 0.45 it was, because a mat that
                // is isotropic under a combed crown reads through it and takes
                // the comb back out.
                .clumps(CLUMP_COUNT, 0.62, 0.44)
                .clump_tint(0.34)
                .midpoint(0.22)
                .facing_variation(1.0)
                // Half of them are buried to the shoulder, which is what makes
                // a mat out of a scatter: the ends stick out and the middles do
                // not.
                .height_offset(FIBRE_LENGTH_METRES * 0.34)
                .length_variation(0.58)
                .width_variation(0.40)
                // Seven tenths of a half turn, where it was the whole of one.
                // A fibre has little grain to keep and the flow field only
                // biases it, but a mat with no grain at all is the speckle this
                // material had: what the reference's ground shows between its
                // blades is more short strokes lying the same way, not noise.
                .direction_variation(0.70)
                .lean_variation(0.30)
                .bend_variation(0.50)
                .segments(FIBRE_SEGMENTS)
                .taper(0.88)
                .root_occlusion(0.20),
        )
}

/// The `stragglers` layer: the few that stand up, and what the rim is made of.
///
/// The two layers above are a mat, and a mat has a smooth edge. Measured on the
/// reference's own silhouette — the radius with the sphere subtracted — it
/// deviates by 5.6 mm and runs out to ten; ours, with `blades` and `fibres`
/// alone, deviates by 3.2 and runs out to five. Looking at the two rims beside
/// each other says the same thing in one word: the reference has *escapes*
/// standing over the crown and we had a nap.
///
/// So this layer is sparse, upright and long. A third of a coarse lattice
/// carries one, it leans between nine and twenty-seven degrees where a blade
/// leans forty, and at the same forty millimetres the mat's tallest blade is
/// cut to it stands nearly twice as far proud. That is the whole of it: 30 216
/// triangles, four per cent of the material's budget, spent entirely on the
/// edge a mat cannot draw. It takes the silhouette's deviation from 3.2 mm to
/// 4.0 and the depth of the fringe from 5.8 mm to 8.3, against the
/// reference's 5.6 and 10.1.
///
/// It is also the drier half of the lawn. A blade that escaped the last cut is
/// the one that went to seed, so its tip is mixed towards `dry_color` rather
/// than towards the crown, which is where the tan flecks over the reference's
/// green come from.
fn stragglers(g: crate::MaterialGraphBuilder, seed: u32) -> crate::MaterialGraphBuilder {
    g
        // A third of the lattice, gathered where the tufts are thick: an escape
        // grows out of a bunch that the mower missed rather than out of bare
        // ground.
        .node(
            "escape_cover",
            remap(
                m(Add, m(Mul, "tufts", 0.58), m(Mul, "clumps", 0.42)),
                0.26,
                0.66,
            ),
        )
        .node(
            "escape_length",
            remap(m(Add, m(Mul, "tufts", 0.5), m(Mul, "mid", 0.5)), 0.78, 1.0),
        )
        // Nine to twenty-seven degrees off the surface normal. This is the one
        // layer in the material that is allowed to stand up, and it pays for it
        // in the relief: a strand this upright splats its own footprint and
        // little more, so what it contributes to the texture is a fleck rather
        // than a stroke. That is the right trade here and the wrong one for the
        // other two, which is why they lean.
        .node("escape_lean", remap("mid", 0.07, 0.24))
        .node("escape_bend", remap("clumps", 0.22, 0.48))
        .node("escape_root_color", m(Mul, "lush_color", 0.86))
        .node(
            "escape_tip_color",
            Mix::new("lush_color", m(Mul, "dry_color", 1.45), p("tip_brightness")),
        )
        .strands(
            "stragglers",
            StrandLayer::new()
                .count(ESCAPE_COUNT)
                .seed(seed.wrapping_add(211))
                .density("escape_cover")
                .length("escape_length")
                .lean("escape_lean")
                .bend("escape_bend")
                .direction("blade_flow")
                .clump("tuft")
                .colors("escape_root_color", "escape_tip_color")
                .roughness("bed_rough")
                .metres(HEIGHT_SCALE_METRES, ESCAPE_WIDTH_METRES)
                .per_cell(1)
                // Sharing the mat's tufts rather than keeping its own, so an
                // escape stands over a bunch that is already tall instead of in
                // the gap beside it.
                .clumps(CLUMP_COUNT, 0.80, 0.24)
                .clump_tint(0.30)
                .midpoint(0.24)
                .facing_variation(0.90)
                .height_offset(0.0)
                .length_variation(0.34)
                .width_variation(0.30)
                .direction_variation(0.75)
                .lean_variation(0.30)
                .bend_variation(0.40)
                .segments(BLADE_SEGMENTS)
                .taper(0.92)
                // Small. It is the thing standing highest in the frame, so
                // almost nothing is above it to do any occluding.
                .root_occlusion(0.10),
        )
}

/// The surface: the same strands, splatted from above.
///
/// Five reliefs of `blades`, four of `fibres` and three of `stragglers`, which
/// is twelve planes over **three** scatters — the plane cache keys them on the
/// layer rather than on the node, so each layer's field evaluations are paid
/// once and the splats are a few million texel writes each.
fn surface(g: crate::MaterialGraphBuilder) -> crate::MaterialGraphBuilder {
    let blade = |output| StrandRelief::new("blades", TILE_METRES).output(output);
    let fibre = |output| StrandRelief::new("fibres", TILE_METRES).output(output);
    let escape = |output| StrandRelief::new("stragglers", TILE_METRES).output(output);
    g.node("relief_cover", blade(StrandReliefOutput::Coverage))
        .node("relief_height", blade(StrandReliefOutput::Height))
        .node("relief_along", blade(StrandReliefOutput::Along))
        .node("relief_color", blade(StrandReliefOutput::Color))
        .node("relief_mass", blade(StrandReliefOutput::Mass))
        .node("fibre_cover", fibre(StrandReliefOutput::Coverage))
        .node("fibre_height", fibre(StrandReliefOutput::Height))
        .node("fibre_color", fibre(StrandReliefOutput::Color))
        .node("fibre_mass", fibre(StrandReliefOutput::Mass))
        // Three rather than five: an escape is sparse and upright, so it has no
        // useful `Along` — the band its tip would draw is a fleck — and its
        // mass is a rounding error beside the mat's. What it does own is the
        // top of the canopy, which is `Height`.
        .node("straggler_cover", escape(StrandReliefOutput::Coverage))
        .node("straggler_height", escape(StrandReliefOutput::Height))
        .node("straggler_color", escape(StrandReliefOutput::Color))
        // The pile both layers stand in.  `Mass` is every contribution summed
        // rather than a union, so it keeps rising where strands cross and it is
        // the honest measure of "how much grass is here"; a blur of the
        // coverage, which is what stood here before, spreads the mass sideways
        // instead of summing it and gives a smear.  Five overlapping strands is
        // a full mat, so that is where the range tops out.
        .node("mass", m(Add, "relief_mass", "fibre_mass"))
        .node("pile", Levels::new("mass").in_range(0.0, 6.0).gamma(0.70))
        // How fully this texel is grass rather than the thing under it, and it
        // is the mass rather than the coverage.  `cover` below is a union and
        // it saturates slowly, so even a closed mat only reaches about half of
        // it, and a mix weighted by that can never reach the strand colour: the
        // old graph's crown was two parts dark ground to one part blade
        // everywhere, which is most of why our sphere came back at half the
        // reference's brightness with the same black point.
        //
        // Two overlapping strands is already a floor you cannot see through, so
        // that is where this tops out.  It is not the same claim as `pile`,
        // which is asking how *deep* the mat is and needs the whole range.
        .node("mat", Levels::new("mass").in_range(0.0, 1.6))
        // How high the canopy stands over a texel, in `0..=1` of the pile: the
        // taller of the two layers' topmost contributions.  A strand does not
        // lie flat, so a texel under a tip reads near one and a texel under a
        // root reads near zero even though both are covered.
        .node(
            "crown",
            Levels::new(m(
                Max,
                m(Max, "relief_height", "fibre_height"),
                "straggler_height",
            ))
            .in_range(0.0, 0.40),
        )
        // How deep in the pile a texel sits, which is the diagnostic worth
        // exporting and the thing the dark is made of.  It is the *floor* of
        // the canopy that is occluded, not the crown standing in the light.
        .node("canopy", Invert::new("crown"))
        // How much strand of either kind covers the texel.  An over-composite
        // rather than a sum, because a fibre lying across a blade does not
        // cover more than all of it.
        .node(
            "cover",
            Clamp::new(m(
                Sub,
                m(Add, "relief_cover", "fibre_cover"),
                m(Mul, "relief_cover", "fibre_cover"),
            )),
        )
        // The standing strand's own height, faded out with the coverage that
        // drew it.  `Height` alone is the *topmost* contribution and nothing
        // else, so it steps from a blade's full height to bare surface across
        // the one texel the blade's edge falls in — and a step like that, at a
        // millimetre a texel and an 18 mm scale, is a vertical wall.  That wall
        // is where the black gouges in the close-up came from: not the colour
        // and not a parallax, but a normal map saturated at every blade edge.
        // Multiplying by the antialiased coverage is what turns the wall back
        // into the ramp the splat already computed.
        .node("ridge", m(Mul, "crown", "cover"))
        // The height, and there is no bare ground in it anywhere.  The pile is
        // most of it — it is an accumulation, so it is smooth — the standing
        // strands ride on top at a sixth of the range, and the noise fields
        // tilt the whole thing over metres and break the floor up under it.
        .node(
            "height",
            Clamp::new(m(
                Add,
                0.06,
                m(
                    Add,
                    m(Add, m(Mul, "patches", 0.05), m(Mul, "mid", 0.05)),
                    m(
                        Add,
                        m(
                            Add,
                            m(Add, m(Mul, "clumps", 0.06), m(Mul, "tufts", 0.11)),
                            m(Mul, "micro", 0.05),
                        ),
                        m(Add, m(Mul, "pile", 0.50), m(Mul, "ridge", 0.15)),
                    ),
                ),
            )),
        )
        // The crown band: where a blade is, and near its tip.  `Along` is
        // weighted by coverage rather than read off whichever contribution
        // stood highest, so a full-width flank counts for more than a hairline
        // tip crossing the same texel, and the band has something to sit on.
        .node(
            "tip_mask",
            m(
                Mul,
                Levels::new("relief_along").in_range(0.20, 0.80),
                m(Mul, "cover", Levels::new("micro").in_range(0.14, 0.74)),
            ),
        )
        // What is under the pile, and it is not soil: it is more grass, lying
        // down, in the shade of the grass standing on it.  So it is the bed's
        // own colour darkened, and how far it is darkened depends on how deep
        // the mat is over it — a thin place is nearly ground and a thick one is
        // matted thatch catching a little light.
        //
        // The flat 0.36 this was is the pale-dark floor the close-up showed
        // between the blades.  Nothing in the reference is a floor at all, and
        // the thing that fixed it is not a brighter constant but the fact that
        // the constant now varies with `pile`.
        .node(
            "under_color",
            Mix::new(
                m(Mul, "lush_color", 0.24),
                m(Mul, "lush_color", 0.86),
                m(Mul, "pile", remap("micro", 0.72, 1.28)),
            ),
        )
        // The strands over it.  Where both layers cover, the blade wins,
        // because a blade is the thing lying on top of the fibre.
        .node(
            "strand_color",
            Mix::new(
                Mix::new("fibre_color", "relief_color", "relief_cover"),
                "straggler_color",
                "straggler_cover",
            ),
        )
        // And the mix up from the ground: nothing on the texel is ground, and
        // two strands' worth of mass is strand.  This is the direction that was
        // wrong two passes ago — the old graph started from a fully lit bed and
        // subtracted where a strand stood, which puts the shadow on the one
        // thing that is in the light — and `mat` rather than `cover` is what
        // lets it arrive.
        .node(
            "crown_color",
            Mix::new("under_color", "strand_color", "mat"),
        )
        .node(
            "tip_color",
            Mix::new(
                "crown_color",
                m(Mul, "crown_ramp", 1.12),
                m(Mul, "tip_mask", p("tip_brightness")),
            ),
        )
        // The dark, and it is the depth rather than the coverage.  Grass is a
        // canopy: what is shaded is what sits low in it, whether that is bare
        // ground or the bottom half of a blade, and what is lit is whatever is
        // standing highest over the texel.
        //
        // The floor is 0.56 where it was 0.36, because `under_color` above now
        // carries the canopy's dark itself.  Multiplying a floor that is
        // already dark by a second one that is darker still is how the whole
        // sphere ended up at half the reference's luminance while matching its
        // black point exactly.
        .node("recess_shade", remap("crown", 0.48, 1.0))
        .node("color", m(Mul, "tip_color", "recess_shade"))
        .node(
            "rough",
            Clamp::new(m(Sub, "bed_rough", m(Mul, "tip_mask", 0.085))).range(0.70, 1.0),
        )
}
