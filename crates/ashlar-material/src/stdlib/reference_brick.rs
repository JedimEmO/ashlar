//! Old worn brickwork, read off ambientCG Bricks097 (a photogrammetry scan).
//! The wall is not a running bond of one brick length: courses wander and vary
//! in thickness, every head joint slides off its lattice line, and the joint is
//! wide and nearly flush.
#![allow(
    clippy::too_many_lines,
    reason = "authored reference graph in dependency order"
)]
use super::reference_support::{finish, m, p, remap};
use crate::{
    Channel, MaterialGraph,
    MathOp::{Add, Div, Floor, Fract, Max, Min, Mul, Sin, Sqrt, Step, Sub},
    Param, PbrOutput,
    nodes::{
        Clamp, Colorize, Decompose, IntensityWarp, Invert, Levels, Math, Mix, Noise, Uv, Voronoi,
        VoronoiOutput,
    },
};

/// Bricks along a course. The scan lays seven to nine per 1024 px, so the
/// lattice is eight and every head joint then slides off it.
const UNITS: f32 = 8.0;
/// Courses across the repeat. The scan lays sixteen per 1024 px.
const COURSES: f32 = 16.0;
/// How far the whole layout drifts, in UV. Seven pixels at 1024: the scan's
/// wall wanders that much across its width. The field carrying it is two slow
/// octaves, because a drift that turns inside a brick is not a wall settling,
/// it is a brick bent: at three octaves of the same amplitude the lattice was
/// locally stretched by half and every long edge came out wavy.
const WANDER: f32 = 0.007;
/// How far a head joint slides off its lattice position, in cells. A brick is
/// bounded by two independent draws, spanning short headers and long stretchers.
/// The wider slide breaks up the regular bond without reordering the joints.
const JITTER: f32 = 0.40;
/// Where the brick face sits in the 40 mm range. The scan's height histogram
/// is compressed and bright — 84% of it inside 0.18 of the range — with the
/// holes as its only dark tail, so the wall is laid high and shallow.
const FACE: f32 = 0.675;
/// The last few pixels of the arris, where the worn edge turns over.
const ARRIS: f32 = 0.026;
/// The rounded-over shoulder inside it. The scan's faces are not flat plates
/// with a chamfer: a brick's top keeps falling for the last few pixels before
/// the arris turns, which is what reads as a worn face. Carried too far it
/// inflates every brick into a pillow and closes the joint over.
const SHOULDER: f32 = 0.013;
/// Height lost per unit of UV past the plan boundary. The face does not drop
/// off a cliff into the joint; it carries on down until the mortar outbids it,
/// and the mortar is what the surface then is. Drawn shallow the brick kept
/// winning five pixels into its own joint and the joint closed over; this is
/// steep enough that the hand-over happens within a pixel or two of the plan
/// and the two surfaces meet at the height they agree on. It is also what
/// keeps the colour joint as wide as the height joint: every pixel the brick
/// wins past its own plan is a pixel taken off the beige net, and at half this
/// the net measured three pixels narrower in the base than in the height.
const SKIRT: f32 = 42.0;
/// How much of a corner's radius the rounding spends up the end of a brick
/// rather than along it. Below about four fifths the rounding reaches so far
/// up the end that the whole long edge is inside the arc and the brick comes
/// out a pill; the scan's bricks are rectangles with their corners knocked
/// off, a little wider on the arris than up the end.
const CORNER_V: f32 = 0.88;
/// The floor of a missing or deeply spalled brick, well below everything else.
const HOLE: f32 = 0.48;
/// The share of bricks that stand a little proud of their neighbours. The
/// scan's highest bricks clear the wall by a millimetre or two on a soft
/// gradient, so this is a common small lift rather than a rare white plate.
const PROUD: f32 = 0.055;

/// Fired clay, a jittered bond and height-owned mortar. Heights span 40 mm.
pub fn brick(seed: u32) -> MaterialGraph {
    // Every draw in the wall is a hash of the cell's own two indices, so the
    // seed has to reach the hash as a number rather than as a generator's
    // seed. Any offset into the sine does; this one is stable for a given u32.
    let hash_seed = f32::from(u16::try_from(seed % 1021).unwrap_or(0)) * 0.1373;
    // The layout drifts as one field, so the lattice, the plan and the per-cell
    // hashes all read the same displaced coordinate: the wall is evaluated at
    // `fract(uv + WANDER * layout)` and nothing is left behind at `uv`.
    let mut g = MaterialGraph::builder("library:brick")
        .param(Param::float("joint_mm", 32.0).range(8.0, 44.0))
        .param(Param::float("erosion_mm", 3.0).range(0.0, 8.0))
        .param(Param::float("mortar_fill", 0.565).range(0.1, 0.80))
        .param(Param::float("relief_mm", 12.0).range(4.0, 20.0))
        .param(Param::float("lime_residue", 0.48).range(0.0, 1.0))
        .param(Param::float("rake_mm", 7.0).range(0.0, 20.0))
        .param(Param::float("missing_bricks", 0.008).range(0.0, 0.2))
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node(
            "layout",
            remap(
                Noise::perlin()
                    .periods(2, 2)
                    .octaves(2)
                    .persistence(0.5)
                    .seed(seed.wrapping_add(10)),
                -1.0,
                1.0,
            ),
        )
        .node("drift", m(Mul, "layout", WANDER))
        .node("wu", Math::unary(Fract, m(Add, "u", "drift")))
        .node("wv", Math::unary(Fract, m(Add, "v", "drift")))
        // The bed lines are not sixteen equal steps. A slow swell up the wall
        // and a wide wave along it bend the *course coordinate itself*, so the
        // row a texel is counted into and the bed joint it is measured from
        // move together and a course comes out whole. Bending the distance
        // inside a fixed row is what tore a straight ledge across the wall
        // wherever the wave pushed a brick past its own lattice line, and left
        // the sliver of the next row's brick above it. Both terms come back to
        // themselves over the repeat, so the wall still meets itself, and both
        // are slow enough that a course runs 54 to 76 px rather than pinching.
        .node("sag", m(Mul, Math::unary(Sin, m(Mul, "wv", 4.0)), 0.055))
        .node(
            "bed_wave",
            remap(
                Noise::perlin()
                    .periods(4, 4)
                    .octaves(2)
                    .persistence(0.45)
                    .seed(seed.wrapping_add(22)),
                -0.125,
                0.125,
            ),
        )
        // One cell to the course in v, which is what makes a course thicker
        // than the one above it: the wave displaces the bed lines by different
        // amounts a course apart, and the wall is laid to a line that was out.
        // It is a displacement of the lattice, not a mark drawn on it, so the
        // noise's own cell edges are nowhere visible.
        .node(
            "bed_rows",
            remap(
                Noise::perlin().periods(4, 16).seed(seed.wrapping_add(25)),
                -0.13,
                0.13,
            ),
        )
        .node(
            "course",
            m(
                Add,
                m(Mul, "wv", COURSES),
                m(Add, "sag", m(Add, "bed_wave", "bed_rows")),
            ),
        )
        .node("r", Math::unary(Floor, "course"))
        .node("y", Math::unary(Fract, "course"))
        // The row index taken back into `0..COURSES`, which is what the hashes
        // below are keyed on: a row that counts past the repeat has to hash as
        // the row it meets at the seam or the wall does not tile.
        .node(
            "rw",
            m(Mul, Math::unary(Fract, m(Div, "r", COURSES)), COURSES),
        )
        // Half-brick bond. The offset rides the wrapped row, so sixteen courses
        // carry the stagger back to a whole cell at the seam.
        .node("s", m(Add, m(Mul, "wu", UNITS), m(Mul, "rw", 0.5)))
        .node("cell", Math::unary(Floor, "s"))
        .node("f", Math::unary(Fract, "s"));

    // One hash per lattice cell, for the four cells that can bound or own this
    // texel. A brick generator used to answer this, but its own rows sit on
    // undisplaced lattice lines while the plan above rides the waved course, so
    // identity and geometry disagreed by up to a fifth of a course and the
    // wall was cut by straight ledges where the two readings changed their
    // minds. Hashing the cell's own indices keeps them the same field. The
    // index is wrapped before it is hashed, so the cell past the seam is the
    // cell at the seam, and every texel that can see a cell hashes it the same
    // way, which is what makes the two sides of a joint agree.
    for (i, k) in [-1.0f32, 0.0, 1.0, 2.0].into_iter().enumerate() {
        g = g.node(
            format!("c{i}"),
            m(
                Mul,
                Math::unary(Fract, m(Div, m(Add, "cell", k), UNITS)),
                UNITS,
            ),
        );
        g = g.node(
            format!("g{i}"),
            Math::unary(
                Fract,
                m(
                    Mul,
                    Math::unary(
                        Sin,
                        m(
                            Add,
                            m(Mul, format!("c{i}"), 0.4213),
                            m(Add, m(Mul, "rw", 0.7549), hash_seed + 0.17),
                        ),
                    ),
                    4371.234,
                ),
            ),
        );
        // The head joint that opens cell `cell + k` stands up to 0.4 cell
        // either side of the lattice line, so a brick is the gap between two
        // such draws and no two neighbours are the same length.
        g = g.node(
            format!("j{i}"),
            m(
                Sub,
                m(
                    Mul,
                    Math::unary(Fract, m(Add, m(Mul, format!("g{i}"), 7.919), 0.31)),
                    2.0 * JITTER,
                ),
                JITTER,
            ),
        );
    }

    let g = g
        // Which of the three cells that reach this texel owns it: a texel left
        // of its own cell's jittered joint belongs to the cell behind, one past
        // the next joint to the cell ahead. Every texel answers off the same
        // four draws, so the two sides of a joint always agree.
        .node("prev", m(Step, "j1", "f"))
        .node("next", m(Step, "f", m(Add, 1.0, "j2")))
        .node(
            "edge_l",
            Mix::new(
                Mix::new("j1", m(Sub, "j0", 1.0), "prev"),
                m(Add, 1.0, "j2"),
                "next",
            ),
        )
        .node(
            "edge_r",
            Mix::new(
                Mix::new(m(Add, 1.0, "j2"), "j1", "prev"),
                m(Add, 2.0, "j3"),
                "next",
            ),
        )
        .node(
            "id_cell",
            Mix::new(Mix::new("g1", "g0", "prev"), "g2", "next"),
        )
        // The owning cell's own index, picked the same way. Anything that has
        // to be one value for a whole brick has to be keyed on this and not on
        // the lattice cell the texel happens to stand in, because a brick
        // crosses a lattice line whenever its right-hand joint slides past it.
        .node(
            "own_c",
            Mix::new(Mix::new("c1", "c0", "prev"), "c2", "next"),
        )
        .node("left", m(Sub, "f", "edge_l"))
        .node("right", m(Sub, "edge_r", "f"))
        .node("run", m(Sub, "edge_r", "edge_l"))
        .node("along", m(Div, "left", "run"))
        .node("dx", m(Div, m(Min, "left", "right"), UNITS))
        // The bed joint is the lattice line itself, which the course already
        // carries the wave of. A per-brick shift of this distance used to sit
        // here as well; it moved a brick off the row it was counted into, so
        // the per-brick share of the bed line is now the two insets below,
        // which move a brick's top and bottom edges without moving its row.
        .node("dy", m(Div, m(Min, "y", Invert::new("y")), COURSES))
        // The owning cell's hash names the brick, spread back over the unit
        // interval. The cell hash already carries the course, so no two rows
        // draw alike and the bottom half of the wall is not a copy of the top.
        .node(
            "brick_id",
            Math::unary(Fract, m(Add, m(Mul, "id_cell", 11.317), 0.137)),
        )
        .node("n_size", Math::unary(Fract, m(Mul, "brick_id", 7.13)))
        .node("n_level", Math::unary(Fract, m(Mul, "brick_id", 23.7)))
        .node("n_tilt", Math::unary(Fract, m(Mul, "brick_id", 3.41)))
        .node("n_corner", Math::unary(Fract, m(Mul, "brick_id", 13.71)))
        .node("n_proud", Math::unary(Fract, m(Mul, "brick_id", 5.77)))
        .node("n_hole", Math::unary(Fract, m(Mul, "brick_id", 31.1)))
        .node("n_bed", Math::unary(Fract, m(Mul, "brick_id", 17.3)))
        .node("n_round", Math::unary(Fract, m(Mul, "brick_id", 19.7)))
        .node("n_spall", Math::unary(Fract, m(Mul, "brick_id", 47.3)))
        // The nominal joint, before the two bricks either side of it have each
        // taken their own bite out of it.
        .node("joint_raw", m(Div, p("joint_mm"), 4000.0))
        // A joint pinches and opens along its own run: the two bricks it lies
        // between were not laid to one gauge, and the mortar took up the
        // difference. Without it every joint ran at one width from end to end
        // and the wall read as a net with bricks dropped into it. One cell per
        // brick and no second octave: at four cells to the brick this term
        // moved a long edge three pixels in and out twice along its own length,
        // which is the ripple the scan's edges never have. A single cell gives
        // a joint that tapers from one end to the other instead.
        .node(
            "joint_wander",
            remap(
                Noise::perlin().periods(8, 16).seed(seed.wrapping_add(27)),
                0.85,
                1.15,
            ),
        )
        .node("joint", m(Mul, "joint_raw", "joint_wander"))
        .node(
            "course_mod",
            remap(
                Noise::value().periods(2, 8).seed(seed.wrapping_add(21)),
                0.88,
                1.12,
            ),
        )
        // Each of a brick's four sides is inset by its own amount, so a joint
        // is the sum of two unrelated draws and runs from a dozen to twenty-odd
        // pixels the way the scan's do, rather than to one ruled width. The
        // two draws are wide apart, because the scan's wall has bricks that
        // all but touch in one course and stand twenty pixels apart in the
        // next; drawn to a narrow band the whole wall came out a tidy net.
        // Let individual sides nearly close a joint while others open it wide.
        // Keep the average inset near the original gauge; widening every joint
        // would replace too much clay with mortar.
        .node("near_left", m(Step, "right", "left"))
        .node("near_low", m(Step, Invert::new("y"), "y"))
        .node(
            "inset_u",
            m(
                Mul,
                "joint",
                Mix::new(
                    remap("n_corner", 0.28, 1.55),
                    remap("n_size", 0.28, 1.55),
                    "near_left",
                ),
            ),
        )
        .node(
            "inset_v",
            m(
                Mul,
                m(Mul, "joint", "course_mod"),
                Mix::new(
                    remap("n_level", 0.24, 1.48),
                    remap("n_tilt", 0.24, 1.48),
                    "near_low",
                ),
            ),
        )
        .node("ix", m(Sub, "dx", "inset_u"))
        .node("iy", m(Sub, "dy", "inset_v"))
        // Knocked-off corners: inside a corner quadrant the plan is the
        // distance to a circle of that corner's own radius, and past it the
        // cell distance comes back unchanged, so the straight middle of a side
        // stays where it was. Two pixels to seventeen is the spread the scan
        // shows — half its corners are still near square — and the squared
        // draw keeps most of them at the bottom of it. Held to one band every
        // brick came out the same rounded rectangle. A radius of half a course, which is
        // where this was, put the whole long edge inside the arc and every
        // brick came out a lozenge with semicircular ends.
        .node(
            "corner_noise",
            Noise::perlin()
                .period(16)
                .octaves(2)
                .seed(seed.wrapping_add(13)),
        )
        // Four corners, four draws. Two of them shared down each end and every
        // brick came out the same rounded rectangle stamped twice; a brick
        // whose lower corner is square and whose upper one is gone is what
        // stops a course reading as a row of identical tiles.
        .node("n_c3", Math::unary(Fract, m(Mul, "brick_id", 61.3)))
        .node("n_c4", Math::unary(Fract, m(Mul, "brick_id", 37.9)))
        .node(
            "corner_r",
            m(
                Mul,
                remap("corner_noise", 0.86, 1.14),
                Mix::new(
                    Mix::new(
                        remap(m(Mul, "n_round", "n_round"), 0.0020, 0.0165),
                        remap(m(Mul, "n_corner", "n_corner"), 0.0018, 0.0150),
                        "near_left",
                    ),
                    Mix::new(
                        remap(m(Mul, "n_c3", "n_c3"), 0.0018, 0.0155),
                        remap(m(Mul, "n_c4", "n_c4"), 0.0020, 0.0145),
                        "near_left",
                    ),
                    "near_low",
                ),
            ),
        )
        // The corner is knocked off further along the brick than up its end,
        // so the arc reads as a worn end and not as a lozenge: the v distance
        // is stretched into the circle and squashed back out of it.
        .node("cdx", m(Max, m(Sub, "corner_r", "ix"), 0.0))
        .node(
            "cdy",
            m(Max, m(Sub, "corner_r", m(Div, "iy", CORNER_V)), 0.0),
        )
        .node(
            "plan_round",
            m(
                Add,
                m(
                    Sub,
                    "corner_r",
                    Math::unary(Sqrt, m(Add, m(Mul, "cdx", "cdx"), m(Mul, "cdy", "cdy"))),
                ),
                m(Max, m(Sub, m(Min, "ix", "iy"), "corner_r"), 0.0),
            ),
        )
        // An edge is never a ruled line. It bows two or three pixels over the
        // brick's whole length, frays at two, and is picked at by the grit at
        // one. The bow is one slow cell per two bricks: at four cells to the
        // brick it was a wave rather than a bow and the long edges rippled
        // like a drawn curve, which is the one thing the scan's edges do not
        // do. What it has is fine raggedness along an edge otherwise where it
        // was laid.
        .node(
            "bow",
            remap(
                Noise::perlin()
                    .periods(4, 16)
                    .octaves(2)
                    .persistence(0.40)
                    .seed(seed.wrapping_add(7)),
                -0.0028,
                0.0028,
            ),
        )
        .node(
            "fray_raw",
            Noise::perlin()
                .periods(128, 128)
                .octaves(3)
                .persistence(0.55)
                .seed(seed.wrapping_add(8)),
        )
        .node(
            "plan",
            m(
                Add,
                m(Add, "plan_round", "bow"),
                m(
                    Add,
                    remap("fray_raw", -0.0016, 0.0016),
                    remap("grit", -0.0010, 0.0010),
                ),
            ),
        )
        .node("support", m(Step, "plan", 0.0))
        // Clay is crumpled at centimetre scale and gritty at millimetre scale.
        .node(
            "mottle",
            Noise::perlin()
                .period(32)
                .octaves(3)
                .persistence(0.65)
                .seed(seed.wrapping_add(1)),
        )
        .node(
            "folds",
            Noise::perlin().periods(32, 128).seed(seed.wrapping_add(2)),
        )
        .node(
            "grit",
            Noise::value().period(512).seed(seed.wrapping_add(3)),
        )
        .node(
            "pits",
            Levels::new(Voronoi::new().period(128).seed(seed.wrapping_add(4)))
                .in_range(0.105, 0.02),
        )
        // Most bricks sit within a millimetre of each other and a few are set
        // back two or three. Nothing here stands far out: the scan's bright
        // tail is a soft lift of a millimetre or two, and reading it as a big
        // spread is what put glaring white plates in the wall.
        // A brick sits a millimetre or two off its neighbours, and a whole course
        // sits off the courses above and below it: old work is laid to a line
        // that was itself out. The course term is a draw on the wrapped row
        // alone, so it steps at the bed joint and never inside a face.
        .node("rise", remap("n_level", -0.076, 0.028))
        .node(
            "course_level",
            remap(
                Math::unary(Fract, m(Add, m(Mul, "rw", 0.3719), 0.21)),
                -0.05,
                0.05,
            ),
        )
        // The wall itself is not a plane: it swells and leans back over a
        // metre, carrying brick and joint together. It reaches the mortar as
        // well as the face, because a bay of wall that stands forward stands
        // forward whole — added to the bricks alone it only sank them into
        // their own joints and the wall read flat with a patch of bad pointing.
        .node(
            "bulge",
            remap(
                Noise::perlin()
                    .periods(2, 2)
                    .octaves(2)
                    .seed(seed.wrapping_add(16)),
                -0.050,
                0.050,
            ),
        )
        // A brick pushed a little out of the wall, rising toward one end. The
        // lift is a millimetre or two on a gradient the whole brick long, so
        // it reads as a brick sitting forward and not as a white tile.
        .node("proud_gate", m(Step, PROUD, "n_proud"))
        .node(
            "proud",
            m(Mul, "proud_gate", m(Add, 0.012, m(Mul, "along", 0.044))),
        )
        .node(
            "recess",
            Levels::new("n_tilt")
                .in_range(0.62, 0.97)
                .out_range(0.0, 0.062),
        )
        // The face is never a plate: it is dished along its length, tilted so
        // one end stands higher than the other, and worn into shallow dips.
        // All three are smooth, because a brick worn by weather is smooth.
        .node(
            "crown",
            m(
                Mul,
                m(Mul, "along", Invert::new("along")),
                remap("n_hole", -0.070, 0.070),
            ),
        )
        .node(
            "tilt",
            m(
                Add,
                m(Mul, m(Sub, "along", 0.5), remap("n_corner", -0.074, 0.074)),
                m(Mul, m(Sub, "y", 0.5), remap("n_size", -0.045, 0.045)),
            ),
        )
        .node(
            "wear",
            remap(
                Noise::perlin()
                    .periods(8, 16)
                    .octaves(3)
                    .persistence(0.50)
                    .seed(seed.wrapping_add(6)),
                -0.018,
                0.018,
            ),
        )
        .node(
            "face_grit",
            m(Sub, remap("grit", -0.0010, 0.0010), m(Mul, "pits", 0.0016)),
        )
        // Spalling: a piece of a face has come away a millimetre or two deep
        // with a ragged border, and on a few bricks it has gone in far enough
        // to read as a hole. The field is slid by the brick's own identity, so
        // a spall stops dead at a joint instead of washing across the wall.
        .node(
            "spall_raw",
            Noise::perlin()
                .periods(16, 16)
                .octaves(2)
                .persistence(0.50)
                .seed(seed.wrapping_add(19)),
        )
        .node(
            "spall_slid",
            IntensityWarp::new("spall_raw", "brick_id")
                .angle(0.373)
                .amount(0.31),
        )
        .node(
            "spall_field",
            m(
                Add,
                m(Add, "spall_slid", remap("fray_raw", -0.07, 0.07)),
                remap("grit", -0.04, 0.04),
            ),
        )
        // Few and large, with a broken rim: a spall is one piece off a face,
        // not a rash. Small cells and a wide ramp gave every brick a handful
        // of soft pale blotches that read as lichen growing on the relief; one
        // cell to two bricks and a ramp two pixels wide gives a patch with a
        // step down one side of it, which is the broken edge. The bricks
        // beside a hole spall hardest, so a hole and the scooped-out half of
        // its neighbour read as one opening a brick and a half wide.
        .node(
            "spall_thr",
            m(
                Sub,
                remap("n_spall", 0.58, 0.97),
                m(Mul, "hole_cluster", 0.10),
            ),
        )
        .node(
            "spall",
            Levels::new(m(Div, m(Sub, "spall_field", "spall_thr"), 0.022)),
        )
        .node(
            "spall_depth",
            m(
                Mul,
                "spall",
                m(
                    Add,
                    0.030,
                    m(Mul, Levels::new("n_spall").in_range(0.90, 0.99), 0.175),
                ),
            ),
        )
        .node(
            "eroded",
            m(
                Mul,
                Levels::new("spall_field").in_range(0.62, 0.88),
                m(Div, p("erosion_mm"), 110.0),
            ),
        )
        .node("relief", m(Div, p("relief_mm"), 12.0))
        .node(
            "modelled",
            m(
                Mul,
                "relief",
                m(
                    Add,
                    m(
                        Sub,
                        m(Add, m(Add, "rise", "course_level"), "proud"),
                        m(Add, "recess", "spall_depth"),
                    ),
                    m(Add, m(Add, "tilt", "crown"), m(Add, "wear", "face_grit")),
                ),
            ),
        )
        .node(
            "face",
            m(Sub, m(Add, m(Add, FACE, "bulge"), "modelled"), "eroded"),
        )
        // The plateau, the worn arris, and the skirt that carries the face on
        // down until the mortar outbids it, so the two meet without a crease.
        // How far the arris has worn is the brick's own draw: a fresh one keeps
        // a square edge and a beaten one rolls right over, and drawing every
        // brick alike put a dark ring round each of them like a floor tile.
        .node("arris_wear", remap("n_size", 0.30, 1.45))
        .node(
            "profile",
            Levels::new("plan").in_range(0.0, 0.0022).gamma(0.6),
        )
        .node(
            "shoulder",
            Levels::new("plan").in_range(0.0, 0.0045).gamma(0.75),
        )
        .node("skirt", m(Max, m(Sub, 0.0, "plan"), 0.0))
        .node(
            "gap",
            m(Max, m(Max, m(Sub, 0.0, "ix"), m(Sub, 0.0, "iy")), 0.0),
        )
        .node(
            "prefill",
            m(
                Sub,
                m(
                    Sub,
                    m(
                        Sub,
                        "face",
                        m(
                            Mul,
                            m(Mul, Invert::new("profile"), ARRIS),
                            m(Mul, "arris_wear", "relief"),
                        ),
                    ),
                    m(
                        Mul,
                        m(Mul, Invert::new("shoulder"), SHOULDER),
                        m(Mul, "arris_wear", "relief"),
                    ),
                ),
                m(Mul, "skirt", SKIRT),
            ),
        )
        // How far into the joint a texel stands, and it has to reach its floor
        // fast. Ramped over six pixels it never reached the floor at all in a
        // twelve-pixel joint: the dish came out a V with a black point down
        // the middle and a slit where the two sides disagreed. Three pixels
        // with the curve bent early gives a joint that is dished across its
        // whole width and only turns up in the last pixel or two against the
        // arris, which is what the scan's pointing does.
        .node(
            "joint_depth",
            Levels::new(m(Div, "gap", 0.0034)).gamma(0.55),
        )
        // One cell per course in v and one per two bricks in u, so a run of
        // open joint follows a single bed for a brick or three and stops. Half
        // a course tall it spilled up and down into the courses either side and
        // the whole net went dirty instead of a few named runs going open; the
        // narrow band on top of it is what keeps the count to the handful the
        // scan shows rather than a quarter of the wall.
        .node(
            "rake_gate",
            Levels::new(m(
                Add,
                Noise::perlin()
                    .periods(4, 16)
                    .octaves(2)
                    .persistence(0.50)
                    .seed(seed.wrapping_add(9)),
                remap("fray_raw", -0.07, 0.07),
            ))
            .in_range(0.48, 0.60),
        )
        // Where a low-frequency field says so the mortar has gone instead, over
        // the joint's whole width: the field is wide in u and half a course
        // thin in v, so an open run follows one bed for a brick or three,
        // frays out at its ends and takes the odd head joint with it. What is
        // left is a recess four or five millimetres deep with a ragged floor —
        // the scan's open joints sit about a sixth of the range under the
        // faces, not the half that reads as a black slot cut in the wall.
        .node(
            "rake",
            m(
                Mul,
                m(Mul, "rake_gate", "joint_depth"),
                m(
                    Mul,
                    m(Div, p("rake_mm"), 40.0),
                    remap("fray_raw", 0.70, 1.30),
                ),
            ),
        )
        // Pointing is not one level. Over a bay or two the mortar was struck
        // flush and the joint all but disappears; over the next it was left
        // low and the brick stands clear of it. Drawn at one height the wall
        // came out as bright tiles in a dark net, which is the one thing the
        // scan is not.
        .node(
            "fill_field",
            remap(
                Noise::perlin()
                    .periods(8, 8)
                    .octaves(2)
                    .persistence(0.55)
                    .seed(seed.wrapping_add(24)),
                -0.042,
                0.042,
            ),
        )
        // Overfilled mortar lapping over a brick's edge: raising the bed in
        // patches lets it win ground from the arris on its own.
        .node(
            "smear",
            Levels::new(
                Noise::perlin()
                    .periods(8, 16)
                    .octaves(2)
                    .seed(seed.wrapping_add(11)),
            )
            .in_range(0.44, 0.86)
            .out_range(0.0, 0.038),
        )
        .node(
            "mortar_raw",
            m(
                Sub,
                m(
                    Add,
                    m(
                        Add,
                        m(Add, p("mortar_fill"), "bulge"),
                        m(Add, "smear", "fill_field"),
                    ),
                    m(
                        Sub,
                        m(
                            Add,
                            remap("mottle", -0.032, 0.024),
                            remap("grit", -0.005, 0.005),
                        ),
                        m(Mul, m(Mul, "joint_depth", 0.030), "relief"),
                    ),
                ),
                "rake",
            ),
        )
        // The mortar can never stand above the brick it was struck against.
        // Without this bound the wall was plastered over in patches: a brick
        // whose own draws all leaned down — set back, tilted, dished and laid
        // to a low course — sank the eleven hundredths that separate the face
        // from the bed, the mortar outbid it across the whole face, and the
        // skim below painted the lot the colour of the joint. A brick-sized
        // field of bare mortar is the one thing the scan never shows: every
        // lattice position carries a brick, and the only openings are the
        // three or four holes. Bounding the mortar rather than narrowing the
        // draws keeps the wall's relief and makes the burial impossible
        // instead of merely unlikely, and it only ever lowers the mortar, so a
        // rise in `mortar_fill` can still only take ground off a brick. It
        // binds nowhere on a brick standing where most of them do, so no step
        // appears down the middle of a joint where its two sides read
        // different bricks.
        // The bound is the worn face itself and not the plateau the brick was
        // laid at. Filling the set-back and the spall back in before bounding
        // — which is what this did — let the mortar rise into a spall and sit
        // there as a ragged cream island in the middle of an otherwise whole
        // face, and a spall is a piece broken off a brick showing raw clay,
        // never a pocket of pointing. What a set-back brick loses by it is
        // that its joints no longer stand proud of it; that reads off the
        // relief between it and its neighbours instead, and a plastered brick
        // is the worse of the two faults by far.
        .node("mortar_height", m(Min, "mortar_raw", m(Sub, "face", 0.028)))
        // Whichever surface stands higher is the surface. Multiplying this by
        // the plan as well used to cut the brick off at its own boundary while
        // its rolled arris was still above the mortar, which is a step down of
        // a millimetre or two drawn in one texel all round every brick. The
        // skirt already carries the face below the mortar within a pixel or
        // two of the plan, so the hand-over happens where the two are equal
        // and the joint meets the arris without a crease.
        .node(
            "brick_mask",
            Levels::new(m(Sub, "prefill", "mortar_height")).in_range(-0.006, 0.006),
        )
        .node("mortar_mask", Invert::new("brick_mask"))
        // Missing bricks are rare at the default amount. Their openings stop
        // at the owning cell so the neighbouring masonry stays intact.
        // Holes gather where the wall has failed, so the draw is one value for
        // a block of four bricks rather than per brick. It is a hash and not a
        // field: a smooth field crossing a face put part of a brick over the
        // threshold and left a hard-edged sliver of hole floor sitting in the
        // middle of an otherwise whole face.
        .node(
            "hole_cluster",
            Levels::new(Math::unary(
                Fract,
                m(
                    Mul,
                    Math::unary(
                        Sin,
                        m(
                            Add,
                            m(Mul, Math::unary(Floor, m(Mul, "own_c", 0.5)), 1.117),
                            m(
                                Add,
                                m(Mul, Math::unary(Floor, m(Mul, "rw", 0.5)), 0.7131),
                                hash_seed + 0.41,
                            ),
                        ),
                    ),
                    2137.13,
                ),
            ))
            .in_range(0.38, 0.68),
        )
        .node(
            "hole_gate",
            m(
                Step,
                m(
                    Mul,
                    p("missing_bricks"),
                    m(Add, 0.30, m(Mul, "hole_cluster", 1.5)),
                ),
                "n_hole",
            ),
        )
        .node(
            "hole_tear",
            Noise::perlin()
                .periods(64, 64)
                .octaves(2)
                .persistence(0.55)
                .seed(seed.wrapping_add(23)),
        )
        .node(
            "hole_edge",
            m(
                Add,
                m(Add, "plan", 0.010),
                m(
                    Sub,
                    m(
                        Add,
                        remap("hole_tear", -0.004, 0.004),
                        remap("spall_slid", -0.008, 0.008),
                    ),
                    m(
                        Mul,
                        Levels::new("along").in_range(0.32, 0.04),
                        remap("n_bed", 0.005, 0.023),
                    ),
                ),
            ),
        )
        // Remove the arris with the face instead of retaining a closed clay rim.
        // A larger remnant survives at one end; low-amplitude tears keep the
        // shoulder from becoming a ring of curled spikes. Fade before the cell
        // boundary so a hole cannot cut into a differently owned neighbour.
        .node(
            "hole_mask",
            m(
                Mul,
                "hole_gate",
                m(
                    Min,
                    Levels::new("hole_edge").in_range(-0.002, 0.007),
                    Levels::new(m(Min, "dx", "dy")).in_range(0.0, 0.003),
                ),
            ),
        )
        // The floor is soft rubble a centimetre or two across, falling away
        // toward the top of the course where the wall behind is in shadow. A
        // finer field than this read as salt and pepper poured into a box.
        .node(
            "rubble",
            Noise::perlin()
                .periods(32, 64)
                .octaves(2)
                .persistence(0.55)
                .seed(seed.wrapping_add(12)),
        )
        .node(
            "hole_floor",
            m(
                Add,
                m(Add, HOLE, remap("n_bed", 0.0, 0.07)),
                m(
                    Add,
                    m(Mul, m(Sub, "y", 0.5), 0.105),
                    m(
                        Add,
                        m(
                            Add,
                            remap("rubble", -0.105, 0.105),
                            remap("hole_tear", -0.015, 0.015),
                        ),
                        remap("mottle", -0.02, 0.02),
                    ),
                ),
            ),
        )
        .node(
            "hole_level",
            Mix::new("surface", m(Min, "surface", "hole_floor"), "hole_mask"),
        )
        // Lime deposits gather in broken islands near the pointing, with fine
        // mineral seams across selected faces. Both share the residue control.
        .node("edge_lime", Levels::new("plan").in_range(0.010, -0.004))
        .node(
            "lime_patch",
            Noise::perlin()
                .period(8)
                .octaves(3)
                .persistence(0.6)
                .seed(seed.wrapping_add(15)),
        )
        // Fine crust gathers near the arris, with a separate deposit level for
        // each brick. The continuous fine fields avoid square value-noise flecks;
        // the narrow ramp leaves translucent, broken margins around the crust.
        // The old threshold sat above the field's default range and hid it all.
        .node(
            "residue_field",
            m(
                Add,
                m(
                    Add,
                    m(Add, m(Mul, "lime_patch", 0.35), m(Mul, "skin_slid", 0.25)),
                    m(Add, m(Mul, "fray_raw", 0.40), m(Mul, "edge_lime", 0.085)),
                ),
                m(
                    Add,
                    remap("pock_raw", -0.14, 0.14),
                    remap("n_salt", -0.08, 0.08),
                ),
            ),
        )
        .node(
            "residue_islands",
            Levels::new(m(
                Div,
                m(
                    Sub,
                    "residue_field",
                    m(Sub, 1.0, m(Mul, p("lime_residue"), 0.76)),
                ),
                0.065,
            )),
        )
        // Thin mineral seams branch across selected faces. Warp the cellular
        // boundary before thresholding so it does not read as a polygon grid.
        .node(
            "crust_cells",
            Voronoi::new()
                .period(128)
                .seed(seed.wrapping_add(46))
                .output(VoronoiOutput::Edge),
        )
        .node(
            "crust_warp",
            IntensityWarp::new("crust_cells", "hole_tear")
                .angle(0.31)
                .amount(0.018),
        )
        .node(
            "crust_web",
            m(
                Mul,
                Levels::new("crust_warp").in_range(0.12, 0.015),
                Levels::new("fray_raw").in_range(0.46, 0.60),
            ),
        )
        .node(
            "crust_patch",
            Levels::new(m(Add, "skin_slid", remap("n_salt", -0.30, 0.30))).in_range(0.61, 0.73),
        )
        .node(
            "residue",
            m(
                Max,
                m(Mul, "residue_islands", 0.72),
                m(
                    Mul,
                    m(Mul, "crust_web", "crust_patch"),
                    Levels::new(m(Div, p("lime_residue"), 0.60)),
                ),
            ),
        )
        .node(
            "surface",
            m(
                Add,
                Mix::new("mortar_height", "prefill", "brick_mask"),
                m(Mul, "residue", 0.002),
            ),
        )
        .node(
            "height",
            Clamp::new(m(Min, "surface", "hole_level")).range(0.02, 0.95),
        )
        // Colour. Five draws off the brick's own identity, so neighbours differ
        // abruptly the way a reclaimed wall's do rather than drifting.
        .node("n_fire", Math::unary(Fract, m(Mul, "brick_id", 29.13)))
        .node("n_shade", Math::unary(Fract, m(Mul, "brick_id", 41.7)))
        .node("n_skin", Math::unary(Fract, m(Mul, "brick_id", 13.03)))
        .node("n_soot", Math::unary(Fract, m(Mul, "brick_id", 53.9)))
        .node("n_salt", Math::unary(Fract, m(Mul, "brick_id", 7.77)))
        // Four clays, all of them red. Sampled at brick centres in the scan —
        // not through a mask, because a mask that leaks mortar reads the
        // joint's ratio of 1.2 back as clay and that is what made the pale
        // family pink — the wall runs 0.07 to 0.12 linear red for its deep
        // maroons, 0.16 to 0.25 for its mid ones, 0.29 to 0.37 for the red
        // ones and 0.42 to 0.53 for the salmon. The ratio of red to green
        // barely moves across that: 2.5 at the bottom, 2.0 at the top. A
        // bright brick here is a hotter-fired red, never a buff or a grey, so
        // the top family is salmon-orange and is a minority. The plateaus make
        // the pick categorical, so a brick is one of the four rather than a
        // point on a ramp between them. Neither the flaked bricks nor the
        // black ones are families here: the pale flakes and the soot are drawn
        // further down. A black family is what made the burnt bricks read as
        // flat cutouts with no red in them.
        .node(
            "clay",
            Colorize::new("n_fire").gradient([
                (0.00, [0.120, 0.042, 0.025]),
                (0.34, [0.120, 0.042, 0.025]),
                (0.36, [0.205, 0.073, 0.039]),
                (0.80, [0.205, 0.073, 0.039]),
                (0.82, [0.292, 0.132, 0.075]),
                (0.90, [0.292, 0.132, 0.075]),
                (0.92, [0.455, 0.212, 0.112]),
                (1.00, [0.455, 0.212, 0.112]),
            ]),
        )
        // A brick is streaked along its own length, which is how it came out of
        // the kiln: few cells across the course and many up the wall, so a mark
        // is a hundred pixels long and a dozen high.
        .node(
            "streak",
            Noise::perlin()
                .periods(8, 64)
                .octaves(2)
                .persistence(0.5)
                .seed(seed.wrapping_add(35)),
        )
        // The face's own variation, slid by the brick so it stops at the joint:
        // an unslid field at this scale reads as a cloud lying over the wall,
        // which is exactly what a fired face does not do.
        .node(
            "clay_grain",
            Noise::perlin()
                .periods(32, 64)
                .octaves(3)
                .persistence(0.52)
                .seed(seed.wrapping_add(40)),
        )
        .node(
            "clay_slid",
            IntensityWarp::new("clay_grain", "brick_id")
                .angle(0.829)
                .amount(0.29),
        )
        // The scan's faces are not pocked like a sponge. They are smooth, with
        // a fine sandy grain over them and the odd small dark pit — a dozen to
        // a face, not a rash. Threshold set at half the field, a quarter of
        // every texel came out a dark hole and every brick carried a regular
        // leopard speckle, which is the one mark the scan's clay does not
        // have. The threshold now sits in the field's low tail, so a tenth of
        // the texels are touched at all and a fiftieth go the whole way down,
        // and the depth is a third of what it was.
        .node(
            "pock_raw",
            Noise::perlin()
                .periods(256, 256)
                .octaves(2)
                .persistence(0.5)
                .seed(seed.wrapping_add(43)),
        )
        .node(
            "pock",
            Levels::new(m(Add, "pock_raw", remap("grit", -0.09, 0.09))).in_range(0.42, 0.28),
        )
        .node(
            "pock_mul",
            m(Sub, 1.0, m(Mul, "pock", remap("n_size", 0.16, 0.38))),
        )
        // Within a family a brick is a shade off its neighbours, and its face
        // carries the crumple, the streak and its own slid grain. Four
        // multipliers compound, so each one is bounded and each leans down
        // rather than up: at a fifth either way apiece the product reached
        // twice and a mid-red brick came out bright orange, which is most of
        // why the wall read light. Cut symmetrically instead and every face
        // went flat, which is worse — the scan's bricks run from near black to
        // bright over their own length. So each range is long on the dark side
        // and short on the light one, and the light end is the bleach below.
        .node(
            "clay_bright",
            m(
                Mul,
                remap("n_shade", 0.78, 1.18),
                m(
                    Mul,
                    Levels::new("mottle")
                        .in_range(0.28, 0.72)
                        .out_range(0.92, 1.06),
                    m(
                        Mul,
                        remap("streak", 0.70, 1.16),
                        remap("clay_slid", 0.82, 1.15),
                    ),
                ),
            ),
        )
        // Where a face has bleached it gets brighter and barely greyer. The
        // scan's palest clay still sits at a ratio near two: a face that
        // loses its red as it lightens is a face with mortar dust on it, and
        // drawn that way across a fifth of the wall it cast the whole thing
        // dusty pink. The threshold stays high all the same, because this is
        // the top fifth of the field and not its upper half.
        .node(
            "bleach",
            Levels::new(m(Add, m(Mul, "clay_slid", 0.55), m(Mul, "mottle", 0.45)))
                .in_range(0.70, 0.90),
        )
        // A face is not one hue lit unevenly. The kiln ran hot down one part
        // of a brick and cool down another, so a patch of orange-red sits in a
        // maroon body and a liver-brown one beside it — which is what gives
        // the scan's clay its richness. Every term above this one was a
        // scalar, so a face could only be a brighter or darker copy of one
        // colour, and the wall came out a single brown-rose at nine levels.
        .node(
            "fire_tint",
            Mix::new(
                [0.92, 1.04, 1.07],
                [1.16, 0.90, 0.78],
                Levels::new("streak").in_range(0.30, 0.72),
            ),
        )
        .node(
            "clay_body",
            Mix::new(
                m(Mul, m(Mul, "clay", "fire_tint"), "clay_bright"),
                m(
                    Mul,
                    m(Mul, m(Mul, "clay", "fire_tint"), "clay_bright"),
                    [1.30, 1.34, 1.30],
                ),
                m(Mul, "bleach", 0.28),
            ),
        )
        // The flaked skin, which is this wall's signature. Islands, not a
        // dither: the field lays cells twice as long as they are high so a
        // patch runs with the course, the threshold gives the crisp border,
        // and the fray tears that border at the pixel and throws off satellite
        // specks. Four octaves of it plus per-texel grit came out as a fine
        // horizontal dash pattern, which is what no scan shows.
        .node(
            "skin_raw",
            Noise::perlin()
                .periods(16, 32)
                .octaves(3)
                .persistence(0.52)
                .seed(seed.wrapping_add(31)),
        )
        .node(
            "skin_slid",
            IntensityWarp::new("skin_raw", "brick_id")
                .angle(0.213)
                .amount(0.37),
        )
        .node(
            "skin_field",
            m(
                Add,
                m(
                    Add,
                    m(Mul, "skin_slid", 0.82),
                    m(Add, m(Mul, "streak", 0.12), m(Mul, "folds", 0.06)),
                ),
                m(
                    Add,
                    remap("fray_raw", -0.050, 0.050),
                    remap("grit", -0.016, 0.016),
                ),
            ),
        )
        // How much skin a brick has left. Five bricks in eight have none at
        // all and read as bare dark body; a fifth carry a couple of islands;
        // the rest are flaked over a third of the face, and one in twenty has
        // gone pale nearly all over. The level is an affine of the field's
        // own threshold, so a stop at one puts the threshold past anything the
        // field reaches and that brick keeps its skin whole. Whole-brick
        // coverage used to be a seventh of the wall and a middling threshold
        // took most of the rest, so about two fifths of the brick area came
        // out pale and the wall read washed. Under a tenth is what the scan
        // shows, and the pale *bricks* it does have are a clay family above,
        // not flakes.
        .node(
            "skin_thr",
            m(
                Sub,
                m(
                    Mul,
                    Decompose::new(
                        Colorize::new("n_skin").gradient([
                            (0.000, [1.0, 1.0, 1.0]),
                            (0.620, [1.0, 1.0, 1.0]),
                            (0.625, [0.610, 0.610, 0.610]),
                            (0.830, [0.610, 0.610, 0.610]),
                            (0.835, [0.565, 0.565, 0.565]),
                            (0.945, [0.565, 0.565, 0.565]),
                            (0.950, [0.521, 0.521, 0.521]),
                            (1.000, [0.521, 0.521, 0.521]),
                        ]),
                        Channel::R,
                    ),
                    1.45,
                ),
                0.25,
            ),
        )
        .node("skin", m(Step, "skin_field", "skin_thr"))
        // What the flake shows. A pale salmon, and it has to be clearly warmer
        // and brighter than the joint beside it or the island reads as mortar
        // smeared on the face — which is what the wall looked like when the
        // skin and the sand sat within a shade of each other. The two part on
        // hue as much as on value: sampled inside the scan's flaked faces the
        // skin runs 0.29 to 0.50 linear red at a red-to-green ratio near two
        // where the mortar beside it sits near 1.2. Drawn at 1.6 the island
        // came out peach on tan and vanished into the joint it was next to;
        // a saturated salmon island against a deep maroon body is what the
        // scan's flaked bricks actually show, and it is this wall's signature.
        // On about a seventh of bricks the flake goes the other way and shows
        // a dark crust over a red body.
        .node(
            "skin_clay",
            Colorize::new("n_shade").gradient([
                (0.00, [0.285, 0.132, 0.072]),
                (0.34, [0.285, 0.132, 0.072]),
                (0.36, [0.385, 0.180, 0.098]),
                (0.72, [0.385, 0.180, 0.098]),
                (0.74, [0.495, 0.235, 0.125]),
                (1.00, [0.495, 0.235, 0.125]),
            ]),
        )
        // The island is a flat plate of skin, not a second cloud: modulated as
        // widely as the body it sat on, its border dissolved and the patch
        // read as a bright smudge rather than as a flake with an edge.
        .node(
            "skin_tone",
            m(
                Mul,
                Mix::new(
                    m(Mul, "skin_clay", remap("n_salt", 0.90, 1.11)),
                    [0.058, 0.028, 0.019],
                    m(Step, "n_salt", 0.86),
                ),
                remap("clay_slid", 0.90, 1.10),
            ),
        )
        .node("clay_flaked", Mix::new("clay_body", "skin_tone", "skin"))
        // Fired clay is sandy at the texel, but the sand is the last thing a
        // face says and not the first. A per-texel multiplier a third either
        // way laid the same pepper over every brick at the same strength, and
        // a wall of faces that are all one grade of sandpaper reads flat
        // however well its colours are picked: the scan's faces are smooth
        // clay in patches, with the grain riding on top of the patches. Cut
        // to an eighth apiece, though, the faces came out as flat plates of
        // paint and the wall lost the sandiness it is made of, so the two
        // terms run a quarter either way and the patch structure above them
        // is what keeps them from reading as one grade of sandpaper.
        .node(
            "clay_pocked",
            m(
                Mul,
                m(
                    Mul,
                    m(Mul, "clay_flaked", "pock_mul"),
                    remap("grit", 0.76, 1.26),
                ),
                remap(Levels::new("grain").in_range(0.30, 0.74), 0.86, 1.18),
            ),
        )
        // Soot. A brick is blackened whole and more heavily toward one of its
        // ends, and which bricks is very nearly its own draw. Laid on as a
        // cloud it was a neutral grey fog drifting over brick and mortar
        // alike, which reads as a smoke sticker and not as a wall; leaning the
        // threshold hard on the same wall-scale field the mortar's dirt reads
        // is the same fault dressed up, because then a dozen neighbouring
        // bricks and the pointing between them all go dark together and the
        // blot crosses every edge in its way. The scan's sooted bricks are
        // scattered singly, so the field only tips the odd extra one over.
        .node(
            "soot_side",
            Mix::new("along", Invert::new("along"), m(Step, 0.5, "n_tilt")),
        )
        // The graded end and the broken crust both used to run most of the way
        // to nothing, so a sooted brick was covered over a third of itself and
        // faded out over the rest — a soft grey smudge lying on a red face,
        // which is the smoke-sticker fault again at brick scale. A brick that
        // has been in a fire is black over nearly all of it and shows its clay
        // through torn gaps, so both terms now start high and the crust breaks
        // at an edge rather than dissolving.
        .node(
            "soot_ramp",
            remap(Levels::new("soot_side").in_range(0.95, 0.05), 0.58, 1.0),
        )
        .node(
            "soot",
            m(
                Mul,
                m(
                    Mul,
                    Levels::new(m(
                        Div,
                        m(Sub, "n_soot", m(Sub, 0.952, m(Mul, "grime", 0.045))),
                        0.06,
                    )),
                    "soot_ramp",
                ),
                // The blackening is blotchy inside the brick as well as graded
                // along it, so the crust breaks and the clay shows through it
                // in patches rather than covering the face like paint.
                remap(Levels::new("clay_slid").in_range(0.34, 0.50), 0.52, 1.0),
            ),
        )
        // The scan's burnt bricks are the wall's darkest ground and they are
        // still bricks: brown-black at one end, red coming through at the
        // other. A multiplier that lifted blue over red — which is what this
        // was — pulled the crust toward slate, and against the saturated clay
        // beside it that reads as a cool stain rather than as burnt brick.
        // Soot is near achromatic and a shade warm, so the clay under it keeps
        // its hue and the brick goes brown-black. Taken down to a quarter of
        // the clay it went past brown-black into a flat dark grey with no
        // colour left to read at all, which against the red beside it looks
        // blue; a third of the clay is as far as a face can go and still be
        // seen as burnt brick.
        .node(
            "clay_soot",
            Mix::new(
                "clay_pocked",
                m(Mul, "clay_pocked", [0.36, 0.30, 0.25]),
                m(Mul, "soot", 0.80),
            ),
        )
        // A dry bloom of salt on some of the dark faces: sparse pale specks
        // gathered in patches, not a wash. The crusty lime in the joint is a
        // separate thing and sits on top of everything. Carried over a fifth
        // of the way to a neutral grey on a smooth ramp it stopped being
        // specks at all: a face went soft grey-brown over half of itself at a
        // red-to-green ratio of 1.2, which is the mortar's ratio, and read as
        // a smudge of dust wiped across the brick. The specks are the grit and
        // the patch has an edge, so both gates are cut hard.
        .node(
            "effl",
            m(
                Mul,
                m(
                    Mul,
                    Levels::new("n_salt").in_range(0.32, 0.39),
                    Levels::new("clay_slid").in_range(0.48, 0.60),
                ),
                Levels::new("grit").in_range(0.80, 0.92),
            ),
        )
        // A brick darkens toward its own worn edges, where a century of dirt
        // has collected in the arris. Without it a face reads as a flat card
        // laid on the wall rather than as a rounded block.
        .node("arris_dirt", Levels::new("plan").in_range(0.0, 0.0085))
        .node(
            "brick_color",
            m(
                Mul,
                Mix::new(
                    Mix::new(
                        "clay_soot",
                        [0.024, 0.015, 0.011],
                        m(
                            Mul,
                            "pits",
                            m(Mul, 0.66, Levels::new("clay_slid").in_range(0.28, 0.66)),
                        ),
                    ),
                    [0.245, 0.214, 0.164],
                    m(Mul, "effl", 0.65),
                ),
                m(Add, 0.74, m(Mul, "arris_dirt", 0.26)),
            ),
        )
        // Mortar: a warm beige-ochre sand, not a cool grey. It goes dirty grey
        // brown where it is recessed or raked and where a grime field says so.
        .node(
            "mortar_patch",
            Noise::perlin()
                .periods(32, 32)
                .octaves(2)
                .persistence(0.5)
                .seed(seed.wrapping_add(41)),
        )
        // Four multipliers compound here too. The scan's mortar has a fine
        // grain of low contrast over it, so each is narrow: a quarter either
        // way apiece put the product between two thirds and two, which is the
        // pebble-dash the joint came out as.
        .node(
            "sand",
            m(
                Mul,
                m(Mul, remap("grit", 0.79, 1.23), remap("mottle", 0.92, 1.09)),
                m(
                    Mul,
                    remap("grain", 0.68, 1.34),
                    remap("mortar_patch", 0.88, 1.14),
                ),
            ),
        )
        // The joint is aggregate, not a fill: pale grains of sand standing in
        // it and the shade between them, both a few pixels across. The shade
        // is a shade and not a hole — drawn as near-black over two fifths of
        // the joint's texels it read as coarse concrete, and the scan's mortar
        // has no texel anywhere near that dark outside a rake.
        .node(
            "aggregate",
            m(
                Max,
                Levels::new("grain").in_range(0.62, 0.77),
                Levels::new("fray_raw").in_range(0.56, 0.70),
            ),
        )
        .node(
            "void",
            m(
                Max,
                Levels::new("grain").in_range(0.32, 0.17),
                Levels::new("fray_raw").in_range(0.34, 0.22),
            ),
        )
        .node(
            "grime",
            Levels::new(
                Noise::perlin()
                    .periods(8, 16)
                    .octaves(2)
                    .persistence(0.55)
                    .seed(seed.wrapping_add(33)),
            )
            .in_range(0.54, 0.80),
        )
        // Dirt is what a raked or weather-beaten joint carries, not the ground
        // state of the pointing. Every joint standing at full depth used to
        // start a third of the way to grey-brown before the grime field was
        // added, which took the whole net down to two thirds the scan's
        // brightness and left the mortar reading as concrete. What an open run
        // reads as in colour is this term going most of the way over: a
        // grey-brown dirt line following one bed for a brick or three, which
        // the scan has several of and a clean cream net has none. The grime
        // field's own share is small and stays small: carried at a quarter it
        // is a soft dark cloud lying across the pointing wherever the field
        // happens to be high, and a cloud with no edge and no joint under it
        // reads as smoke blown onto the photograph. The scan's mortar drifts
        // between cream and grey-beige over whole bays — which is the `bay`
        // multiplier below — and goes properly dirty only where the joint is
        // open.
        .node(
            "dirt",
            Clamp::new(m(
                Add,
                m(Mul, "joint_depth", 0.10),
                m(
                    Add,
                    m(Mul, "grime", 0.09),
                    m(Mul, "rake_gate", m(Add, 0.34, m(Mul, "grime", 0.34))),
                ),
            )),
        )
        // A run of joint a couple of bricks long is bright struck sand and the
        // next is weathered grey-green, so the net is not one tone drawn round
        // every brick; the scan's joints differ that much from bay to bay.
        .node(
            "joint_tone",
            Levels::new(
                Noise::perlin()
                    .periods(8, 16)
                    .octaves(3)
                    .persistence(0.55)
                    .seed(seed.wrapping_add(36)),
            )
            .in_range(0.34, 0.66),
        )
        // Clumps of sand a few pixels across, under the per-texel grit: the
        // scan's mortar is visibly aggregate, not a flat fill with noise on it.
        // The same field pocks the clay, so wall and joint share one grain.
        .node(
            "grain",
            Noise::perlin()
                .period(256)
                .octaves(2)
                .persistence(0.5)
                .seed(seed.wrapping_add(37)),
        )
        // Where the mortar was struck proud it is pale cream; where it sits
        // back it is dirtier and greyer. The smear field already lifts those
        // patches in the height, so the colour follows the same lump.
        .node("struck", Levels::new("smear").in_range(0.006, 0.030))
        // Whole bays of pointing were made up on different days and have
        // weathered differently since: one is cream and the next is a grey
        // sand two thirds as bright, over a run wider than any single joint.
        // It rides the mortar alone — laid over the wall it is a fog across
        // the brick faces, which is the one thing this material keeps having
        // to be cured of.
        .node(
            "bay",
            remap(
                Noise::perlin()
                    .periods(2, 4)
                    .octaves(2)
                    .persistence(0.5)
                    .seed(seed.wrapping_add(45)),
                0.86,
                1.14,
            ),
        )
        // Measured off joints in the scan the mortar runs 0.41 to 0.50 red,
        // 0.28 to 0.41 green, 0.21 to 0.26 blue in linear where it is clean
        // and about half that where it is weathered — a warm cream sand,
        // brighter than every brick in the wall, and never a cool grey.
        .node(
            "mortar_base",
            m(
                Mul,
                Mix::new(
                    Mix::new([0.238, 0.190, 0.130], [0.522, 0.414, 0.262], "joint_tone"),
                    [0.578, 0.480, 0.320],
                    m(Mul, "struck", 0.42),
                ),
                "bay",
            ),
        )
        // A thin dark line where the mortar meets the brick, strongest in the
        // bed joint under a brick, where the arris shades what is below it.
        // Three or four pixels of it, and on some bricks only: carried a
        // third of the way across the joint and drawn round every brick alike
        // it is a cast shadow rather than collected dirt, and the wall read as
        // cut-out bricks dropped onto a mortar sheet with the net a third
        // narrower than the height says it is.
        .node(
            "shadow_band",
            Levels::new("plan").in_range(-0.0060, -0.0012),
        )
        .node(
            "edge_shadow",
            m(
                Mul,
                m(Mul, "shadow_band", m(Step, 0.0, "plan")),
                m(
                    Mul,
                    m(Add, 0.30, m(Mul, Invert::new("near_low"), 0.70)),
                    m(
                        Mul,
                        remap("joint_tone", 0.70, 1.15),
                        remap("n_bed", 0.10, 1.00),
                    ),
                ),
            ),
        )
        // Weathered growth: the joint goes green-grey in the bays where water
        // runs, which is a tint on the mortar rather than a colour of its own.
        .node(
            "lichen",
            Levels::new(
                Noise::perlin()
                    .periods(8, 8)
                    .octaves(3)
                    .persistence(0.55)
                    .seed(seed.wrapping_add(39)),
            )
            .in_range(0.48, 0.66),
        )
        .node(
            "mortar_color",
            m(
                Mul,
                Mix::new(
                    Mix::new(
                        m(
                            Mul,
                            Mix::new(
                                Mix::new(
                                    Mix::new(
                                        "mortar_base",
                                        [0.780, 0.690, 0.412],
                                        m(Mul, "aggregate", 0.62),
                                    ),
                                    [0.098, 0.084, 0.060],
                                    m(Mul, "void", 0.60),
                                ),
                                [0.044, 0.040, 0.033],
                                m(Mul, "pits", 0.40),
                            ),
                            Mix::new([1.0, 1.0, 1.0], [0.88, 0.98, 0.74], "lichen"),
                        ),
                        [0.132, 0.106, 0.072],
                        "dirt",
                    ),
                    [0.072, 0.058, 0.042],
                    m(Mul, "edge_shadow", 0.62),
                ),
                "sand",
            ),
        )
        // Mortar was smeared over the brick when it was struck: the colour of
        // the joint carries a few pixels onto the face with a ragged border,
        // which is why the brick outline in the scan is nowhere a clean line.
        .node(
            "lap_noise",
            Noise::perlin()
                .periods(16, 32)
                .octaves(2)
                .seed(seed.wrapping_add(34)),
        )
        // Two or three pixels of it, over part of a brick's perimeter. At five
        // pixels and half the perimeter every brick sat in a pale band and the
        // wall read as bricks pressed into plaster, which is the opposite of
        // what the lap is for.
        .node(
            "lap",
            m(
                Mul,
                m(Step, 0.0026, "plan"),
                m(
                    Step,
                    m(Add, "lap_noise", remap("grit", -0.10, 0.10)),
                    remap("n_size", 0.62, 0.98),
                ),
            ),
        )
        .node("face_mask", m(Max, m(Sub, "brick_mask", "lap"), 0.0))
        // Where the mortar has won ground over a brick's own plan it is a thin
        // dirty skim, not the struck face of the joint: without this the smears
        // read as bright cream washes lying across the courses. The joint net
        // itself is never touched: `support` is the inside of the plan, and an
        // open joint is outside every plan there is.
        .node("skim", m(Mul, "support", Invert::new("face_mask")))
        .node(
            "wall_color",
            Mix::new(
                Mix::new(
                    "mortar_color",
                    m(Mul, "mortar_color", [0.66, 0.66, 0.65]),
                    "skim",
                ),
                "brick_color",
                "face_mask",
            ),
        )
        // The bloom is crusty lime and it sits on top of both. It is a warm
        // cream, not a white one, and it is sparse: drawn near-white over a
        // tenth of the wall it put cream plates over whole brick faces, which
        // is the plastered look the joint colour had already been cured of.
        .node(
            "lime",
            m(Mul, [0.360, 0.330, 0.245], remap("pock_raw", 0.55, 1.25)),
        )
        .node(
            "bloom_color",
            Mix::new("wall_color", "lime", m(Mul, "residue", 0.82)),
        )
        // The back of a hole is dirty mortar and rubble. Drawn as one dark
        // warm grey it came out a flat neutral rectangle — the one thing in
        // the wall with no structure in it, and at a glance it read as a hole
        // punched in the image rather than in the wall. What the scan shows is
        // a range as wide as the wall's own: broken bed mortar catching the
        // light at the bottom of the opening, brown rubble in the middle, and
        // the top of the course all but black where the wall behind it is in
        // shadow. So the three tones part further and the dust is nearly
        // mortar-bright where it lands.
        .node(
            "hole_deep",
            Levels::new(m(
                Add,
                m(Add, 0.40, m(Mul, m(Sub, 0.5, "y"), 1.65)),
                remap("rubble", -0.50, 0.50),
            )),
        )
        // The dust lies where the broken bed is, which is the bottom of the
        // opening: carried over the whole floor it is a grey wash and the
        // rubble under it stops reading as lumps at all.
        .node(
            "hole_dust",
            m(
                Mul,
                m(Mul, Levels::new("mortar_patch").in_range(0.46, 0.70), 0.60),
                Invert::new("hole_deep"),
            ),
        )
        // The lumps have to shade like lumps — a broad light and dark to the
        // rubble field on top of the tint — or a hole is a flat card of grit
        // whatever colour it is painted.
        .node(
            "hole_lump",
            remap(Levels::new("rubble").in_range(0.34, 0.76), 0.42, 1.62),
        )
        .node(
            "hole_face",
            m(
                Mul,
                Mix::new(
                    Mix::new([0.115, 0.076, 0.046], [0.014, 0.010, 0.008], "hole_deep"),
                    [0.185, 0.142, 0.086],
                    "hole_dust",
                ),
                m(
                    Mul,
                    m(Mul, "hole_lump", "pock_mul"),
                    m(Mul, remap("grain", 0.62, 1.36), remap("grit", 0.76, 1.24)),
                ),
            ),
        )
        .node("color", Mix::new("bloom_color", "hole_face", "hole_mask"))
        // Roughness. The scan is matte throughout — its median is 0.59 and its
        // quartiles 0.54 and 0.64 — with the mortar the rougher of the two, the
        // flaked skin rougher than the body it sits on, and the burnt and
        // sooted bricks the only thing approaching a sheen. Both grounds carry
        // about twice the texel-scale variation the colour does: measured
        // against the scan, a face varies by 0.039 inside a five-pixel window
        // and a joint by 0.074, so the fine terms here are wide on purpose.
        .node(
            "rough_brick",
            m(
                Add,
                m(
                    Add,
                    remap("n_shade", 0.545, 0.625),
                    m(
                        Add,
                        m(Mul, "skin", 0.095),
                        remap("clay_slid", -0.050, 0.050),
                    ),
                ),
                m(
                    Sub,
                    m(
                        Add,
                        m(
                            Add,
                            remap("grit", -0.055, 0.055),
                            remap("mottle", -0.035, 0.035),
                        ),
                        m(Add, m(Mul, "effl", 0.11), m(Mul, "pock", 0.055)),
                    ),
                    m(
                        Add,
                        m(Mul, Levels::new("n_fire").in_range(0.20, 0.18), 0.13),
                        m(Add, m(Mul, "pits", 0.07), m(Mul, "soot", 0.14)),
                    ),
                ),
            ),
        )
        // The scan's joints are its brightest and its noisiest ground: crusty
        // sand, not a smooth plate a shade lighter than the brick. Drawn as
        // noisy as the grit alone it read as television static.
        .node(
            "rough_mortar",
            m(
                Add,
                m(Sub, 0.805, m(Mul, "dirt", 0.17)),
                m(
                    Add,
                    m(
                        Add,
                        remap("grit", -0.115, 0.115),
                        remap("grain", -0.065, 0.065),
                    ),
                    m(
                        Sub,
                        m(Add, m(Mul, "residue", 0.08), m(Mul, "aggregate", 0.09)),
                        m(Add, m(Mul, "edge_shadow", 0.13), m(Mul, "void", 0.10)),
                    ),
                ),
            ),
        )
        .node(
            "rough",
            Clamp::new(Mix::new(
                Mix::new("rough_mortar", "rough_brick", "face_mask"),
                remap("pock_raw", 0.78, 0.96),
                "hole_mask",
            ))
            .range(0.12, 1.0),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("rough")
            .metallic(0.0)
            .occlusion("ao")
            .extra("brick_id", "brick_id")
            .extra("brick_mask", "brick_mask")
            .extra("mortar_mask", "mortar_mask")
            .extra("hole_mask", "hole_mask")
            .extra("brick_along", "along")
            .extra("joint_depth", "joint_depth")
            .extra("pre_fill_height", "prefill")
            .extra("lime_residue", "residue")
            .extra("flaked_skin", "skin")
            .extra("face_colour", "face_mask"),
        0.04,
    )
}
