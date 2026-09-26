//! Weathering compounds: what time does to a surface somebody else authored.
//!
//! Each of these reads a substrate through [`GraphInput`] nodes, decides where
//! something happened, and hands back the channels it changed plus the mask it
//! decided with. None of them draws a surface of its own: a compound with
//! nothing wired in bakes its own defaults, which is a picture of the
//! weathering and not of a wall.

use crate::{
    Channel, Input, MaterialGraph, MathOp, Param, PbrOutput,
    nodes::{
        Blur, Clamp, Curvature, Decompose, Distance, EdgeDetect, GraphInput, Invert, Levels, Math,
        Mix, Noise, OcclusionFromHeight, SlopeMode, Uv, Voronoi, VoronoiOutput,
    },
    stdlib::{
        CURVATURE_GAIN, DIELECTRIC, FLAT_HEIGHT, MATT, MID_GREY, RELIEF, UNBIASED, bias_input,
        metallic_input, relief_of, substrate, thresholded,
    },
};

/// The radius the crest filter reads, in UV.
///
/// Six millimetres of a metre-and-a-half repeat, which is the brick's arris
/// radius: a knock takes about that much off a corner, and a curvature read
/// wider than the feature it is looking for answers the shape of the wall
/// rather than the shape of the edge.
const CREST_WIDTH: f32 = 0.006;

/// How far a chip is dragged off the edge it started on, in UV, and in how
/// many steps.
///
/// A centimetre, walked in six: far enough that a chip is a patch on the flank
/// rather than a line along the arris, short enough that it stays a chip. The
/// step count is the same the brick's own chips use; more steps buy a smoother
/// tail and each one costs a pass over the plane.
const DRAG_REACH: f32 = 0.01;
const DRAG_STEPS: u32 = 6;

/// The lattice the breaking noise is laid on, and the seed it is hashed with.
///
/// Thirty-two cells over the repeat, two octaves: coarse enough that a chip is
/// a stretch of arris rather than a dotted line along all of it, fine enough
/// that one edge has several. The seed is a field of the node and so cannot be
/// a parameter; a caller who wants a different break instances the compound
/// through a [`Warp`](crate::nodes::Warp) of the height, which moves the whole
/// sampling frame.
const BREAK_PERIOD: u32 = 32;
const BREAK_SEED: u32 = 41;

/// How far the breaking noise is allowed to close a crest.
///
/// A floor rather than zero: the noise decides where the wear is *heaviest*,
/// and an arris that nothing has touched at all is a wall nobody has walked
/// past. The threshold below is what actually takes the wear away.
const BREAK_FLOOR: f32 = 0.3;

/// Paint, patina and fired skin knocked off the edges that stick out.
///
/// Wear is found rather than painted, the way the study's brick finds its
/// arrises: nothing here is told where an edge is. [`Curvature`] over the
/// caller's own height says where the surface stands proud of its
/// neighbourhood, a coarse Perlin breaks that line into knocks rather than a
/// stripe down every edge, and a slope [`Blur`] against the *inverted* height
/// drags each knock downhill onto the flank below it, which is the direction a
/// barrow takes a corner. What comes out is a mask, and the mask mixes the
/// substrate's colour, roughness and metalness in over the finish and cuts
/// `depth` out of the height.
///
/// The substrate defaults are bare steel under a dielectric finish, because
/// that is the case where the wear has to read: a chipped painted panel shows
/// a bright metal edge, and a compound whose defaults made the substrate
/// another mid grey would bake to a picture of nothing happening. A masonry
/// caller binds its own fresh clay or lime into the three `substrate_*` inputs
/// and gets the brick's chip back.
///
/// `bias` is where the caller says wear is possible at all — a
/// [`WorldMask`](crate::nodes::WorldMask) for the faces a hand can reach, a
/// constant for a surface worn all over.
#[must_use]
pub fn edge_wear() -> MaterialGraph {
    let builder = MaterialGraph::builder("weathering:edge_wear")
        // How much of what the crest filter found is actually worn. The
        // default takes the sharpest half of the edges, which is a surface
        // that has been in use rather than one that has been abused.
        .param(Param::float("amount", 0.45).range(0.0, 1.0))
        // The band between "untouched" and "worn through", in the units of the
        // crest field. A tenth is a chip with a lip; a hundredth is a scar cut
        // with a knife, and reads as one.
        .param(Param::float("softness", 0.12).range(0.01, 1.0))
        // How much height a chip takes off, as a fraction of the caller's
        // `0..=1` field. A tenth of the relief is a millimetre of a
        // centimetre-deep surface: a chip, not a hole.
        .param(Param::float("depth", 0.1).range(0.0, 1.0))
        // What the caller's height means as a length. See `stdlib::RELIEF`.
        .param(Param::float("relief", RELIEF).range(0.0, 0.5));
    let builder = bias_input(metallic_input(substrate(builder)))
        // Bare steel: brighter, far smoother and fully metallic, which is what
        // makes a chip read at a distance.
        .node(
            "substrate_color",
            GraphInput::color("substrate_color", BARE_STEEL),
        )
        .node(
            "substrate_roughness",
            GraphInput::float("substrate_roughness", STEEL_ROUGHNESS),
        )
        .node(
            "substrate_metallic",
            GraphInput::float("substrate_metallic", 1.0),
        )
        // Where the surface stands proud of itself, read as a length so that a
        // caller who declares a shallow relief wears less than one who
        // declares a deep one.
        .node("relief_uv", relief_of("height", "relief"))
        .node(
            "crest",
            Curvature::peaks("relief_uv")
                .radius(CREST_WIDTH)
                .strength(CURVATURE_GAIN),
        )
        // Knocks, not a stripe: the crest line is modulated before it is
        // thresholded, so what survives the threshold is stretches of edge.
        .node(
            "knocks",
            Noise::perlin()
                .period(BREAK_PERIOD)
                .octaves(2)
                .persistence(0.55)
                .seed(BREAK_SEED),
        )
        .node("break", Levels::new("knocks").out_range(BREAK_FLOOR, 1.0))
        .node("chipped", Math::new(MathOp::Mul, "crest", "break"))
        // A slope blur carries its source *uphill* along the field it walks
        // down, so the guide is the height turned over: the chip travels down
        // the flank, away from the arris it came off, and a hollow below the
        // edge collects what the edge lost.
        .node("downhill", Invert::new("height"))
        .node(
            "dragged",
            Blur::slope("chipped", "downhill")
                .radius(DRAG_REACH)
                .steps(DRAG_STEPS),
        )
        .node("biased", Math::new(MathOp::Mul, "dragged", "bias"));
    thresholded(
        builder,
        "mask",
        "biased",
        Input::param("amount"),
        Input::param("softness"),
    )
    // Everything below is a `Mix` by the mask, so a mask of zero is the
    // substrate the caller handed in, bit for bit.
    .node(
        "worn_color",
        Mix::new("base_color", "substrate_color", "mask"),
    )
    .node(
        "worn_roughness",
        Mix::new("roughness", "substrate_roughness", "mask"),
    )
    .node(
        "worn_metallic",
        Mix::new("metallic", "substrate_metallic", "mask"),
    )
    .node("cut", Math::new(MathOp::Mul, "mask", Input::param("depth")))
    .node("lowered", Math::new(MathOp::Sub, "height", "cut"))
    // A height is a `0..=1` field and the cut can take it below zero when a
    // caller beds their relief at the floor, so it is held rather than
    // trusted: a height map that leaves the unit is a normal map that is wrong
    // where it clipped.
    .node("worn_height", Clamp::new("lowered"))
    .output(
        PbrOutput::new()
            .base_color("worn_color")
            .roughness("worn_roughness")
            .metallic("worn_metallic")
            .height("worn_height")
            // What this graph bakes alone with. Instanced, the caller's own
            // output carries the strength, and this is ignored.
            .normal_strength(RELIEF)
            .extra("mask", "mask"),
    )
    .into_graph()
}

/// How far the shelter filter looks for something standing over a texel, in UV.
///
/// Two centimetres of the repeat: a joint, a chip floor or a moulding's
/// underside collects dust, and a hollow wider than this has open sky over its
/// middle and keeps dust only round its edges, which is what a swept floor
/// looks like.
const SHELTER_REACH: f32 = 0.02;

/// How much of the sky a texel has to lose before it counts as fully
/// sheltered.
///
/// The horizon march answers how open a texel is over the whole hemisphere,
/// and even the floor of a raked joint keeps a good deal of sky: a mask that
/// only reached one in a crevice would leave an ordinary wall clean.
const SHELTER_FULL: f32 = 0.5;

/// The radius the cavity filter reads, in UV.
///
/// Narrower than the shelter, because the two are looking for different
/// things: the horizon march finds the wide hollows and this finds the tight
/// ones the march walks straight over.
const CAVITY_WIDTH: f32 = 0.005;

/// The lattice the drift is laid on, and its seed.
///
/// Eight cells over the repeat and three octaves: dust does not settle evenly,
/// it drifts, and the scale of a drift is a good fraction of a wall. See
/// `edge_wear`'s `BREAK_SEED` for why a seed is a constant here.
const DRIFT_PERIOD: u32 = 8;
const DRIFT_SEED: u32 = 42;

/// How far the drift is allowed to sweep a hollow clean.
///
/// As with the chip's break: the noise says where the dust is deepest, and the
/// `amount` threshold is what takes it away entirely.
const DRIFT_FLOOR: f32 = 0.25;

/// Dust and dry dirt settled wherever the surface gave it somewhere to sit.
///
/// The deciding question is shelter, and it is asked twice because one filter
/// cannot answer it at both scales. [`OcclusionFromHeight`] marches a horizon
/// and finds the wide hollows — a raked joint, the floor of a chip, the inside
/// of a moulding — while [`Curvature`] finds the tight ones the march steps
/// over. The two are combined by a max rather than an average, because dust
/// sits wherever *either* kind of shelter exists and a hollow that only one of
/// them can see is still a hollow. A low-frequency Perlin then makes it a
/// drift rather than a uniform film.
///
/// It changes the colour and the roughness and nothing else. Dust does not
/// move the surface — a film is microns on a wall whose relief is centimetres
/// — so this compound binds no height, and a caller that reads one is refused
/// rather than handed a height it did not ask for. It is also why the mask is
/// exported as `cavity_mask`: a caller that wants the same dirt darkening its
/// own occlusion map layers on the mask instead of building a second one.
#[must_use]
pub fn dirt_dust() -> MaterialGraph {
    let builder = MaterialGraph::builder("weathering:dirt_dust")
        // How dirty the surface is. Higher than the chip's, because the
        // shelter field is a product of two terms that are each usually well
        // under one and dust is not fussy: what a hollow offers is generally
        // enough for some of it.
        .param(Param::float("amount", 0.75).range(0.0, 1.0))
        // Dust has no edge, so the band is wide where the chip's is narrow.
        .param(Param::float("softness", 0.25).range(0.01, 1.0))
        // A warm pale grey, lighter than most substrates and far duller: dry
        // dust scatters and does not reflect.
        .param(Param::color("color", [0.20, 0.185, 0.16]))
        .param(Param::float("dust_roughness", 0.96).range(0.0, 1.0))
        // What the caller's height means as a length. See `stdlib::RELIEF`.
        .param(Param::float("relief", RELIEF).range(0.0, 0.5));
    let builder = bias_input(substrate(builder))
        .node("relief_uv", relief_of("height", "relief"))
        // The horizon march answers how *open* a texel is, so the shelter is
        // what it did not answer.
        .node(
            "open",
            OcclusionFromHeight::new("relief_uv")
                .radius(SHELTER_REACH)
                .strength(1.0),
        )
        // Half the sky closed over is as sheltered as dust needs: past that a
        // hollow is a hollow, and holding the rest of the range for the
        // deepest crevice on the wall would leave every joint on it clean.
        .node(
            "sheltered",
            Levels::new(Invert::new("open")).in_range(0.0, SHELTER_FULL),
        )
        .node(
            "hollow",
            Curvature::cavity("relief_uv")
                .radius(CAVITY_WIDTH)
                .strength(CURVATURE_GAIN),
        )
        .node("cavity", Math::new(MathOp::Max, "sheltered", "hollow"))
        .node(
            "drift",
            Noise::perlin()
                .period(DRIFT_PERIOD)
                .octaves(3)
                .persistence(0.55)
                .seed(DRIFT_SEED),
        )
        .node("patchy", Levels::new("drift").out_range(DRIFT_FLOOR, 1.0))
        .node("settled", Math::new(MathOp::Mul, "cavity", "patchy"))
        .node("biased", Math::new(MathOp::Mul, "settled", "bias"));
    thresholded(
        builder,
        "cavity_mask",
        "biased",
        Input::param("amount"),
        Input::param("softness"),
    )
    .node(
        "dusted",
        Mix::new("base_color", Input::param("color"), "cavity_mask"),
    )
    .node(
        "dusty",
        Mix::new("roughness", Input::param("dust_roughness"), "cavity_mask"),
    )
    .output(
        PbrOutput::new()
            .base_color("dusted")
            .roughness("dusty")
            .extra("cavity_mask", "cavity_mask"),
    )
    .into_graph()
}

/// Bare steel under a finish: brighter than most substrates, far smoother, and
/// fully metallic.
///
/// The default of every `substrate_color` here, and the reason a chip or a tear
/// reads at a distance. A masonry caller binds its own fresh clay or lime over
/// it.
const BARE_STEEL: [f32; 3] = [0.56, 0.57, 0.58];

/// The roughness of that steel: a rolled surface, not a polished one.
const STEEL_ROUGHNESS: f32 = 0.35;

/// What counts as inside a mask a caller hands in.
///
/// [`Distance`] takes its threshold as a field, so the number is written here
/// once and the hard mask beside it uses the same one: the distance plane and
/// the bare mask have to agree about where the boundary is, or the lip band
/// would be measured from an edge that is not where the colour changes.
const SEED_THRESHOLD: f32 = 0.5;

/// The furthest rust is allowed to creep out of its seed, in UV.
///
/// A quarter of the repeat, and it is the [`Distance`] node's `range` — a field
/// of the node, so no parameter can reach it. What a parameter *can* do is
/// decide where inside that reach the front stands, which is what `spread` is:
/// the distance plane is measured once out to here and the parameter reads a
/// contour off it. The range doubles as the ceiling on `spread`, because past
/// it the plane saturates and the front would stop moving.
const RUST_REACH: f32 = 0.25;

/// The lattice the bloom is laid on, its seed, and how far it is allowed to
/// hold the rust back.
///
/// Sixteen cells over the repeat and three octaves: rust advances as a blotch
/// rather than as a contour, and the scale of a blotch is a hand's width. The
/// floor is high because this noise decides where the scale is *thickest* and
/// not whether there is any — what decides that is the distance from the seed.
/// See `edge_wear`'s `BREAK_SEED` for why a seed is a constant here.
const BLOOM_PERIOD: u32 = 16;
const BLOOM_SEED: u32 = 43;
const BLOOM_FLOOR: f32 = 0.35;

/// The lattice the pits are laid on, its seed, and how much of a cell one eats.
///
/// Sixty-four cells over a metre-and-a-half repeat is a pit every couple of
/// centimetres, which is what a plate that has been rusting for a decade looks
/// like close to. The [`Voronoi`] distance is one at the cell boundary and zero
/// at its point, so the pit is what is left after the levels take the outer
/// half of every cell away: a crater about a third of a cell across rather than
/// a honeycomb of every cell at once.
const PIT_PERIOD: u32 = 64;
const PIT_SEED: u32 = 44;
const PIT_LIP: f32 = 0.55;

/// What iron oxide does to a surface's finish: it is matt and it is not a
/// metal.
///
/// Rust scatters almost everything that reaches it, and the conductor is gone —
/// what is left is an oxide, which is a dielectric. Both matter more than the
/// colour does: a rusted patch that kept the steel's metalness reads as dirty
/// chrome under any light.
const RUST_ROUGHNESS: f32 = 0.93;

/// Iron oxide creeping out of wherever the finish has already failed.
///
/// Rust does not appear where a noise happens to be bright; it appears where
/// water has reached bare metal, and then it spreads from there. So this
/// compound is the one that takes a *place* rather than an amount: `seed_mask`
/// is where the steel is already open — the bare mask of a
/// [`peeling_paint`], the chip mask of an [`edge_wear`], a scratch field, a
/// band of [`Noise`] along a gutter line — and everything else is measured from
/// it. [`Distance`] answers how far each texel is from the nearest seeded one,
/// `spread` reads a contour off that plane, and a coarse bloom makes the front
/// a blotch rather than a contour line.
///
/// The three things it then does are the three things rust does. The colour
/// runs from a bright orange at the advancing front to a dark brown-black in
/// the core, because fresh oxide and old scale are different materials and a
/// single rust colour is the commonest way a metal reads as painted. A
/// [`Voronoi`] distance eats pits into the core, which is where the metal has
/// actually gone. And the film lifts along the front — the band where the mask
/// is neither nothing nor everything — because rust occupies more volume than
/// the iron it came from and what is left of the paint stands over it.
///
/// With nothing wired into `seed_mask` the whole surface is seeded, which is a
/// bake of a plate rusted through rather than of a wall. That is the honest
/// default: a constant input cannot draw a boundary, so a standalone bake shows
/// the compound's terminal state and the edge is what a caller wires in.
#[must_use]
pub fn rust() -> MaterialGraph {
    let builder = MaterialGraph::builder("weathering:rust")
        // How far the front has crept out of the seed, in UV. Eight
        // millimetres of a metre-and-a-half repeat: a rust halo round a chip,
        // which is a surface that has been wet a few winters rather than one
        // that has been abandoned. Bounded by `RUST_REACH`, past which the
        // distance plane saturates and the front would stop moving.
        .param(Param::float("spread", 0.08).range(0.0, RUST_REACH))
        // How much of the caller's `0..=1` height a pit takes at its deepest.
        // Rather more than a chip, because a pit is the metal gone rather than
        // the finish gone.
        .param(Param::float("pitting", 0.15).range(0.0, 1.0))
        // How far the film lifts along the front, in the same units. Smaller
        // than the pitting: a blister is the coat standing over new oxide, and
        // the coat is thin.
        .param(Param::float("blister", 0.06).range(0.0, 1.0))
        // Fresh oxide at the front. Orange, and bright enough to read against
        // a dark finish under a single light.
        .param(Param::color("edge_color", [0.175, 0.062, 0.020]))
        // Old scale in the core: darker, browner and nearly flat, which is
        // what separates a patch that is spreading from one that has stopped.
        .param(Param::color("core_color", [0.045, 0.022, 0.014]));
    let builder = metallic_input(substrate(builder))
        // Where the steel is already open. One rather than zero for the
        // reason `stdlib::UNBIASED` gives, and see this function's own doc
        // comment for what a constant can and cannot show.
        .node("seed_mask", GraphInput::float("seed_mask", UNBIASED))
        // How far each texel is from the nearest open one, as a fraction of
        // `RUST_REACH`, and then in UV.
        .node(
            "from_seed",
            Distance::new("seed_mask")
                .threshold(SEED_THRESHOLD)
                .range(RUST_REACH),
        )
        .node("reached", Math::new(MathOp::Mul, "from_seed", RUST_REACH))
        // The contour `spread` reads off that plane: one at the seed, zero at
        // `spread` from it, and held there. A `spread` of exactly zero divides
        // by zero, which the crate answers with zero — no rust at all, which
        // is what asking for a front of no width should mean.
        .node(
            "under_front",
            Math::new(MathOp::Sub, Input::param("spread"), "reached"),
        )
        .node(
            "creep",
            Math::new(MathOp::Div, "under_front", Input::param("spread")),
        )
        .node("crept", Clamp::new("creep"))
        .node(
            "bloom",
            Noise::perlin()
                .period(BLOOM_PERIOD)
                .octaves(3)
                .persistence(0.55)
                .seed(BLOOM_SEED),
        )
        .node("blotch", Levels::new("bloom").out_range(BLOOM_FLOOR, 1.0))
        // The mask, and everything below is a `Mix` or a multiply by it: past
        // `spread` from the seed `crept` is exactly zero, so the surface is
        // the caller's, bit for bit.
        .node("rust_mask", Math::new(MathOp::Mul, "crept", "blotch"))
        // Where the metal has gone. The pits are cut only where the scale is,
        // so a halo that has just arrived is not already full of holes.
        .node(
            "cells",
            Voronoi::new()
                .period(PIT_PERIOD)
                .seed(PIT_SEED)
                .output(VoronoiOutput::Distance),
        )
        .node(
            "pit",
            Levels::new(Invert::new("cells")).in_range(PIT_LIP, 1.0),
        )
        .node("pit_at", Math::new(MathOp::Mul, "pit", "rust_mask"))
        .node(
            "eaten",
            Math::new(MathOp::Mul, "pit_at", Input::param("pitting")),
        )
        // The front, as a band: a mask times its own inverse peaks at a half
        // and is zero at both ends, which is exactly the line between paint
        // and scale. Four to bring the peak back to one.
        .node("behind_front", Invert::new("rust_mask"))
        .node("band", Math::new(MathOp::Mul, "rust_mask", "behind_front"))
        .node("paint_line", Math::new(MathOp::Mul, "band", 4.0))
        // Broken on the same bloom the mask is, so a blister stands where the
        // oxide behind it is thickest rather than evenly along the line.
        .node("blistering", Math::new(MathOp::Mul, "paint_line", "blotch"))
        .node(
            "lift",
            Math::new(MathOp::Mul, "blistering", Input::param("blister")),
        )
        .node("raised", Math::new(MathOp::Add, "height", "lift"))
        .node("pitted", Math::new(MathOp::Sub, "raised", "eaten"))
        // Held, for the reason `edge_wear` holds its own: a lift and a cut
        // over somebody else's field can leave the unit, and a height map that
        // leaves the unit is a normal map that is wrong where it clipped.
        .node("rust_height", Clamp::new("pitted"));
    builder
        // Orange at the front, dark scale in the core.
        .node(
            "oxide",
            Mix::new(
                Input::param("edge_color"),
                Input::param("core_color"),
                "rust_mask",
            ),
        )
        .node("rusted", Mix::new("base_color", "oxide", "rust_mask"))
        .node(
            "rust_rough",
            Mix::new("roughness", RUST_ROUGHNESS, "rust_mask"),
        )
        .node("rust_metal", Mix::new("metallic", DIELECTRIC, "rust_mask"))
        .output(
            PbrOutput::new()
                .base_color("rusted")
                .roughness("rust_rough")
                .metallic("rust_metal")
                .height("rust_height")
                // What this graph bakes alone with; instanced, the caller's
                // own output carries the strength.
                .normal_strength(RELIEF)
                .extra("rust_mask", "rust_mask"),
        )
        .into_graph()
}

/// The furthest a curl is allowed to reach back from a tear, in UV.
///
/// [`Distance`]'s range again, and the ceiling on `curl_width` for the same
/// reason `RUST_REACH` is the ceiling on `spread`. Eight centimetres of the
/// repeat is far more curl than any paint has; the range is generous because a
/// coarse plane costs the same as a fine one and a parameter that saturates
/// halfway up its own slider is a worse fault than a range nobody uses.
const CURL_REACH: f32 = 0.08;

/// How wide the shadow line under a torn edge is, and how dark it goes.
///
/// A quarter of a centimetre: a curl lifts off the substrate by about the
/// thickness of the film, and what it casts into the gap is a line rather than
/// a gradient. The depth is most of the way to black because the gap under a
/// lifted edge sees almost no sky, and this is the one term a normal map cannot
/// carry — the surface is not bent there, it is undercut.
const TEAR_WIDTH: f32 = 0.0025;
const TEAR_SHADE: f32 = 0.55;

/// The finish of a coat of paint: semi-gloss, the way maintenance paint goes on.
const PAINT_GLOSS: f32 = 0.5;

/// What a coat of paint defaults to when nothing is wired in.
///
/// The study's own panel blue, so that a standalone bake and the showcase's
/// painted metal read as the same material.
const PAINT_COLOR: [f32; 3] = [0.030, 0.072, 0.115];

/// Paint that has lost its key and is coming off in sheets.
///
/// Like [`rust`], this takes a place rather than an amount: `peel_mask` says
/// where the film has already gone, and what the compound adds is everything
/// that happens at the *edge* of that patch, which is the part an author cannot
/// paint by hand and the part that makes a tear read as a tear.
///
/// [`Distance`] measures outward from the bare patch and `curl_width` reads a
/// band off it, on the paint side only — inside the patch there is no film left
/// to lift. Across that band the film curls up, so `lip` is added to the
/// caller's height there and nowhere else. [`EdgeDetect`] over the same
/// boundary is the undercut: the gap between a lifted edge and the substrate
/// under it, written into the occlusion because it is shadow rather than shape
/// and a derived normal has no way to say it.
///
/// What it deliberately does *not* do is drop the height inside the patch by
/// the thickness of the film. A caller's height is the surface they authored,
/// film and all, and a compound that subtracted a coat thickness would be
/// guessing at a number only they know; the visible half of a peel is the lip,
/// and the lip is what this adds.
#[must_use]
pub fn peeling_paint() -> MaterialGraph {
    MaterialGraph::builder("weathering:peeling_paint")
        // How far the film stands proud where it has curled, as a fraction of
        // the caller's `0..=1` height. Larger than a chip's depth: a curl is
        // the coat standing on its edge rather than lying down.
        .param(Param::float("lip", 0.08).range(0.0, 1.0))
        // How far back from the tear the curl reaches, in UV. A centimetre and
        // a bit of a metre-and-a-half repeat, which is a film that has let go
        // rather than one that is merely cracked.
        .param(Param::float("curl_width", 0.012).range(0.0, CURL_REACH))
        .node("paint_color", GraphInput::color("paint_color", PAINT_COLOR))
        .node(
            "substrate_color",
            GraphInput::color("substrate_color", BARE_STEEL),
        )
        .node(
            "paint_roughness",
            GraphInput::float("paint_roughness", PAINT_GLOSS),
        )
        .node(
            "substrate_roughness",
            GraphInput::float("substrate_roughness", STEEL_ROUGHNESS),
        )
        .node("height", GraphInput::float("height", FLAT_HEIGHT))
        // Where the film has already gone. One rather than zero, for the
        // reason `stdlib::UNBIASED` gives and with the caveat `rust` states: a
        // constant cannot draw a boundary, so alone this bakes bare substrate
        // and the tear is what a caller wires in.
        .node("peel_mask", GraphInput::float("peel_mask", UNBIASED))
        // The hard mask, on the same threshold the distance plane uses so that
        // the colour changes exactly where the band is measured from.
        .node("bare", Math::new(MathOp::Step, "peel_mask", SEED_THRESHOLD))
        .node(
            "from_bare",
            Distance::new("peel_mask")
                .threshold(SEED_THRESHOLD)
                .range(CURL_REACH),
        )
        .node("outward", Math::new(MathOp::Mul, "from_bare", CURL_REACH))
        .node(
            "inside_curl",
            Math::new(MathOp::Sub, Input::param("curl_width"), "outward"),
        )
        .node(
            "curl_ramp",
            Math::new(MathOp::Div, "inside_curl", Input::param("curl_width")),
        )
        .node("near_tear", Clamp::new("curl_ramp"))
        // The distance is zero inside the patch as well as at its edge, so the
        // band is cut back to the film: there is nothing to curl where the
        // paint has gone.
        .node("film", Invert::new("bare"))
        .node("curl", Math::new(MathOp::Mul, "near_tear", "film"))
        .node(
            "lifted",
            Math::new(MathOp::Mul, "curl", Input::param("lip")),
        )
        .node("curled", Math::new(MathOp::Add, "height", "lifted"))
        .node("peeled_height", Clamp::new("curled"))
        // The undercut. `EdgeDetect` answers one on a step edge whatever its
        // radius, so the radius is the width of the line and the shade is how
        // dark the gap under the film goes.
        .node("tear", EdgeDetect::new("bare").radius(TEAR_WIDTH))
        .node("shadow", Math::new(MathOp::Mul, "tear", TEAR_SHADE))
        .node("undercut", Invert::new("shadow"))
        .node(
            "exposed",
            Mix::new("paint_color", "substrate_color", "bare"),
        )
        .node(
            "exposed_rough",
            Mix::new("paint_roughness", "substrate_roughness", "bare"),
        )
        .output(
            PbrOutput::new()
                .base_color("exposed")
                .roughness("exposed_rough")
                .occlusion("undercut")
                .height("peeled_height")
                .normal_strength(RELIEF)
                .extra("bare", "bare"),
        )
        .into_graph()
}

/// The roughness of standing water: optically smooth, and the reason a puddle
/// is a mirror.
///
/// Not zero. A perfectly smooth surface is a specular highlight the size of a
/// point, which reads as a bug under a directional light; two hundredths is the
/// varnish-like lobe a real film of water on a rough substrate gives.
const POOLED_ROUGHNESS: f32 = 0.02;

/// How much of the roughness a fully porous substrate loses when it is merely
/// damp.
///
/// The design's figure. A damp surface is not a wet one: the water is *in* the
/// stone rather than on it, so the microfacets are still there and what changes
/// is that the pores are full. Four tenths is most of the way from cast
/// concrete to a sealed floor, and a `porosity` of zero — glass, glazed tile,
/// painted steel — leaves the finish exactly where it was.
const POROSITY_REACH: f32 = 0.4;

/// The depth of water a damp face carries, in the height field's own units.
///
/// The darkening is an absorption over a depth, and a face above the waterline
/// has no depth at all — so without this a damp wall would be exactly as light
/// as a dry one and only the puddles would read. A twentieth of the unit is the
/// film that clings to a vertical surface in rain: enough to darken it visibly,
/// far less than the pooling below it.
const DAMP_FILM: f32 = 0.05;

/// Rain, standing where the surface let it and clinging where it did not.
///
/// The whole of this compound is one observation: water darkens a mineral
/// surface and smooths it, and *where* it does so is decided by the surface's
/// own height rather than by any field an author could paint. So `level` is a
/// waterline in the caller's `0..=1` height — everything below it is submerged,
/// everything above it is merely damp — and `wet` is how much rain has arrived
/// at all, which is the input a caller drives from a
/// [`WorldMask`](crate::nodes::WorldMask), a live parameter or a weather system.
///
/// The three channels it moves are the three that separate wet from dry:
///
/// - **Roughness.** Under the puddle the surface a fragment sees is the water,
///   so the finish is the water's: `POOLED_ROUGHNESS`. Off it the substrate
///   keeps its own microfacets and loses only what its pores took, which is
///   what `porosity` says.
/// - **Colour.** Light that goes into a wet surface comes back less of it, and
///   it falls away with depth — which is an absorption, so the albedo is
///   multiplied by `exp2(-darkening * depth)` through [`MathOp::Exp2`] rather
///   than mixed towards a dark colour. A deep corner therefore goes darker than
///   a shallow one for free, which is the cue that reads as water rather than
///   as somebody having turned the albedo down.
/// - **Height.** Raised to the waterline where submerged, because the surface
///   of a puddle is flat. That is worth more than it costs: the normal a caller
///   derives from the height is then flat across every puddle, and a flat
///   normal under a near-mirror roughness is what a still surface is.
///
/// Metalness goes to zero under the puddle for the same reason the roughness
/// does — what is on top is water, and water is a dielectric — and is left
/// alone where the surface is only damp, which is a film thin enough to see the
/// metal through.
///
/// The one thing it does not read is a neighbourhood of the height, so unlike
/// [`edge_wear`] and [`dirt_dust`] it takes no `relief`: every filter here is
/// pointwise, which is also why a live `wet` costs a handful of instructions in
/// a shader and freezes nothing.
#[must_use]
pub fn moisture() -> MaterialGraph {
    let builder = MaterialGraph::builder("weathering:moisture")
        // Where the waterline stands in the caller's `0..=1` height. A third
        // of the way up: the hollows and the joints hold water and the faces
        // between them do not, which is where rain actually sits on a wall.
        .param(Param::float("level", 0.35).range(0.0, 1.0))
        // The width of the waterline, in the same units. Narrow — a water
        // surface has an edge, and a wide ramp here reads as a stain rather
        // than as a puddle.
        .param(Param::float("softness", 0.05).range(0.01, 1.0))
        // How fast water darkens the albedo per unit of depth, as the exponent
        // of a base-two absorption. At this a texel a third of the unit under
        // the line keeps about half its albedo, which is what a wet flagstone
        // photographs at against its dry half.
        .param(Param::float("darkening", 2.5).range(0.0, 8.0))
        // How much of the surface is pore rather than solid. A half is cast
        // concrete or fired clay; zero is glass, glaze or a sound coat of
        // paint, and a zero here is a damp surface whose finish does not move.
        .param(Param::float("porosity", 0.5).range(0.0, 1.0));
    let builder = metallic_input(substrate(builder))
        // How much rain has arrived. One rather than zero for the reason
        // `stdlib::UNBIASED` gives: a compound with nothing wired into its
        // gate is a compound the caller wants everywhere, and the design's
        // zero would make a standalone bake a picture of a dry wall.
        .node("wet", GraphInput::float("wet", UNBIASED))
        // How far below the waterline a texel is, which the shared threshold
        // helper says exactly: with the depth below the top of the unit as the
        // field and `level` as the amount, `clamp((level - height) / softness)`
        // is submergence, and `level` keeps the meaning every other `amount`
        // in this library has.
        .node("depth_below", Invert::new("height"));
    thresholded(
        builder,
        "pooled",
        "depth_below",
        Input::param("level"),
        Input::param("softness"),
    )
    .node("wet_mask", Math::new(MathOp::Mul, "pooled", "wet"))
    .node("proud", Invert::new("pooled"))
    .node("damp", Math::new(MathOp::Mul, "proud", "wet"))
    // How much water stands over a texel, in the height field's own units,
    // plus the film everything else carries. This is the depth the absorption
    // reads, so it is the one place the two halves meet.
    .node(
        "below_level",
        Math::new(MathOp::Sub, Input::param("level"), "height"),
    )
    .node("under_water", Clamp::new("below_level"))
    .node("pool_depth", Math::new(MathOp::Mul, "under_water", "wet"))
    .node("film_depth", Math::new(MathOp::Mul, "damp", DAMP_FILM))
    .node("water", Math::new(MathOp::Add, "pool_depth", "film_depth"))
    .node(
        "absorbed",
        Math::new(MathOp::Mul, "water", Input::param("darkening")),
    )
    .node("exponent", Math::new(MathOp::Mul, "absorbed", -1.0))
    .node("shade", Math::unary(MathOp::Exp2, "exponent"))
    .node("soaked", Math::new(MathOp::Mul, "base_color", "shade"))
    // The finish, in two passes: the pores fill wherever it is damp, and the
    // water's own surface takes over wherever it pooled.
    .node(
        "filled",
        Math::new(MathOp::Mul, Input::param("porosity"), POROSITY_REACH),
    )
    .node("kept", Invert::new("filled"))
    .node(
        "damp_roughness",
        Math::new(MathOp::Mul, "roughness", "kept"),
    )
    .node("damped", Mix::new("roughness", "damp_roughness", "damp"))
    .node("wetted", Mix::new("damped", POOLED_ROUGHNESS, "wet_mask"))
    .node(
        "submerged_metal",
        Mix::new("metallic", DIELECTRIC, "wet_mask"),
    )
    // No clamp: a `Mix` of two values inside the unit is inside the unit, so
    // unlike `edge_wear`'s cut and `rust`'s lift there is nothing here that
    // could leave it.
    .node(
        "flooded",
        Mix::new("height", Input::param("level"), "wet_mask"),
    )
    .output(
        PbrOutput::new()
            .base_color("soaked")
            .roughness("wetted")
            .metallic("submerged_metal")
            .height("flooded")
            .normal_strength(RELIEF)
            // The puddle rather than the whole wetness: the damp term is the
            // `wet` input the caller already has, and the only thing this
            // compound decided is where the water stood.
            .extra("wet_mask", "wet_mask"),
    )
    .into_graph()
}

/// How far below its source a run-off streak can reach, in UV, and in how many
/// steps the walk up to that source is taken.
///
/// Six tenths of the repeat: a streak off a ledge runs most of the way down a
/// storey and fades before the next ledge's, which is the rhythm a rained-on
/// tower has. Thirty-two steps is a step of about two centimetres of a 3.8 m
/// repeat, and the source is dilated by a step before the walk so a thin sill
/// is caught on every row rather than every other.
const RUN_REACH: f32 = 0.6;
const RUN_STEPS: u32 = 32;

/// How far the source is grown before the walk, in UV: a little over one
/// step of it, so no walk can step over a source however thin.
#[expect(
    clippy::cast_precision_loss,
    reason = "a step count of at most MAX_SLOPE_STEPS, exact in an f32"
)]
const GROWTH: f32 = 2.0 * RUN_REACH / RUN_STEPS as f32;

/// The lattices the streaks are drawn on, and their seeds.
///
/// Thirty-two columns across the repeat, three octaves, one cell up it:
/// run-off follows the same few paths down a wall every time it rains, so a
/// streak is a column that holds its place top to bottom and wanders only
/// slowly, and the octaves give wide stains and thin trickles both. A broad
/// eight-column field says which stretches of a ledge shed the most. The fine
/// texture inside a streak is thirty-two times finer across than along.
const COLUMN_PERIOD: u32 = 32;
const COLUMN_SEED: u32 = 43;
const SHED_PERIOD: u32 = 8;
const SHED_SEED: u32 = 44;
const TEXTURE_PERIODS: [u32; 2] = [128, 4];
const TEXTURE_SEED: u32 = 45;

/// How dark the sheet between the streaks is, as a share of a full streak.
///
/// Water leaves a ledge along all of it, not only down the paths, so the band
/// under a source is dirtied evenly and the streaks stand out of that.
const SHEET: f32 = 0.3;

/// Dirty water run down a wall from wherever it left a ledge.
///
/// Rain that lands on a ledge, a sill or a slab edge picks up soot and dust
/// and leaves by the same few paths every time, so what builds up below it is
/// a set of long vertical streaks, darkest at the ledge and fading as the
/// water spreads and dries. `source` says where the water leaves — a band
/// under a slab edge, a sill mask — and everything else is measured from it:
/// a [`Distance`] plane says how far below the source a texel is, and a slope
/// [`Blur`] walking straight *up* the repeat says whether there is a source
/// above it at all, so nothing streaks upwards or out to the side of a sill.
/// The column field then decides which paths the water takes.
///
/// Down is towards smaller `v`, which is what a box-mapped vertical face
/// carries: `v` is the building's height. The walk reads its direction off
/// `1 - v`, which jumps at the repeat's edge, so a streak never runs across the
/// bottom of the repeat into the top; a caller tiles a storey per repeat and
/// puts its sources inside it.
///
/// `amount` is the contour over the fading run, so it is how far down the
/// streaks reach — a tenth is a stain under the ledge, one is the full
/// `RUN_REACH`, six tenths of the repeat. `opacity` is how dark the darkest of it gets. It changes the
/// colour and the roughness and nothing else: a film of soot is no relief.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn streaks() -> MaterialGraph {
    let builder = MaterialGraph::builder("weathering:streaks")
        // How far down the wall the streaks run, as a contour.
        .param(Param::float("amount", 0.6).range(0.0, 1.0))
        // Wide: a streak fades as the water spreads, it does not stop.
        .param(Param::float("softness", 0.5).range(0.01, 1.0))
        .param(Param::float("opacity", 0.85).range(0.0, 1.0))
        // Soot, near black and faintly warm.
        .param(Param::color("color", [0.014, 0.013, 0.012]))
        .param(Param::float("streak_roughness", 0.88).range(0.0, 1.0))
        .node("base_color", GraphInput::color("base_color", MID_GREY))
        .node("roughness", GraphInput::float("roughness", MATT))
        .node("source", GraphInput::float("source", UNBIASED));
    let builder = bias_input(builder)
        // The source grown by one step of the walk, so a sill thinner than a
        // step is still landed on. A distance rather than a blur because it
        // keeps the grown edge hard.
        .node(
            "grown",
            Distance::new("source").threshold(0.5).range(GROWTH),
        )
        // Hard, because the walk below reads a height off it: a soft edge
        // would carry half a height and read as a source far above.
        .node("caught", Math::new(MathOp::Step, 0.5, "grown"))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("up", Invert::new("v"))
        // Each caught texel carries its own height, lifted clear of zero, so
        // the highest one a walk passes is a height and not only a yes: the
        // distance down from it is then plain arithmetic. A distance plane
        // would measure it too, but over the torus, and a band near the top
        // of the repeat is then also near its bottom.
        .node(
            "lifted",
            Math::new(MathOp::Mul, "caught", Math::new(MathOp::Add, "v", 1.0)),
        )
        .node(
            "highest",
            Blur::slope("lifted", "up")
                .radius(RUN_REACH)
                .steps(RUN_STEPS)
                .slope_mode(SlopeMode::Max),
        )
        .node("under", Math::new(MathOp::Step, "highest", 0.5))
        .node(
            "drop",
            Math::new(MathOp::Sub, Math::new(MathOp::Sub, "highest", 1.0), "v"),
        )
        .node(
            "columns",
            Levels::new(
                Noise::perlin()
                    .periods(COLUMN_PERIOD, 1)
                    .octaves(3)
                    .seed(COLUMN_SEED),
            )
            .in_range(0.32, 0.52),
        )
        .node(
            "shed",
            Levels::new(Noise::perlin().periods(SHED_PERIOD, 1).seed(SHED_SEED))
                .out_range(0.6, 1.0),
        )
        // How far down the water has come: one at the source, falling to zero
        // at the reach a little faster than a line, as water that spreads and
        // dries does, and nothing where no source stands above. The walk
        // finds the source only to within one of its steps, which leaves the
        // fade in stairs; a vertical blur of two steps smooths them.
        .node(
            "stepped",
            Math::new(
                MathOp::Mul,
                "under",
                Math::new(
                    MathOp::Pow,
                    Clamp::new(Math::new(
                        MathOp::Sub,
                        1.0,
                        Math::new(MathOp::Div, "drop", RUN_REACH),
                    )),
                    1.5,
                ),
            ),
        )
        .node("run", Blur::directional("stepped", 90.0).radius(GROWTH));
    let builder = thresholded(
        builder,
        "reached",
        "run",
        Input::param("amount"),
        Input::param("softness"),
    );
    builder
        // A faint sheet under the whole source, and the streaks over it.
        .node(
            "paths",
            Math::new(
                MathOp::Add,
                SHEET,
                Math::new(
                    MathOp::Mul,
                    Math::new(MathOp::Mul, "columns", "shed"),
                    1.0 - SHEET,
                ),
            ),
        )
        .node(
            "texture",
            Levels::new(
                Noise::perlin()
                    .periods(TEXTURE_PERIODS[0], TEXTURE_PERIODS[1])
                    .octaves(2)
                    .seed(TEXTURE_SEED),
            )
            .out_range(0.7, 1.0),
        )
        .node(
            "reached_paths",
            Math::new(
                MathOp::Mul,
                Math::new(MathOp::Mul, "reached", "paths"),
                "bias",
            ),
        )
        .node("mask", Math::new(MathOp::Mul, "reached_paths", "texture"))
        .node(
            "stain",
            Math::new(MathOp::Mul, "mask", Input::param("opacity")),
        )
        .node(
            "sooted",
            Mix::new("base_color", Input::param("color"), "stain"),
        )
        .node(
            "wetted",
            Mix::new("roughness", Input::param("streak_roughness"), "stain"),
        )
        .output(
            PbrOutput::new()
                .base_color("sooted")
                .roughness("wetted")
                .extra("mask", "mask"),
        )
        .into_graph()
}
