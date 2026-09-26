//! Pattern recipes: the fields that are a handful of nodes rather than a node.
//!
//! Each of these draws a surface of its own rather than changing somebody
//! else's, which is the one way they differ from the compounds next door. A
//! pattern takes no substrate, so it declares no channel inputs; it binds the
//! channels that make its own picture; and the field it was built for is
//! exported as an extra, because what a caller usually wants from a gradient or
//! a crack is the *field*, to wire into a [`Mix`], a [`Warp`] or one of the
//! compounds, rather than the picture a standalone bake makes of it.
//!
//! They are recipes rather than nodes on purpose. A radial gradient is a
//! [`CircleMap`] over a [`Uv`] axis, wood is a wave along one axis with two
//! noises pushing it about, and cracks are a [`Voronoi`] boundary walked
//! outward by a [`Distance`]; each is a handful of nodes, and a node per recipe
//! would be a vocabulary that grew every time somebody wanted a different
//! arrangement of the same arithmetic. What they are for is to be instanced,
//! tested and read: a graph here is also the worked example of the construction
//! it is made of.

use crate::{
    Channel, Input, MaterialGraph, MathOp, Param, PbrOutput,
    nodes::{
        CircleMap, Clamp, Decompose, Distance, GraphInput, Invert, Levels, Math, Mix, Noise, Uv,
        Voronoi, VoronoiOutput, Warp,
    },
    stdlib::{MATT, RELIEF, UNBIASED, thresholded},
};

/// The widest disc that fits in the repeat it is centred in, in UV.
///
/// Both gradients are read through a [`CircleMap`], which is a *windowed*
/// resampler: a polar frame over the whole repeat does not meet itself at the
/// seam at all, and a disc inside the repeat with a fill round it does. So the
/// window is the inscribed disc, the four corners past it carry the node's
/// `outside` fill, and that fill is chosen so the field is continuous at the
/// rim rather than stepping there.
const WINDOW: f32 = 0.5;

/// A ramp from the centre of the repeat outward, between two radii the caller
/// chooses.
///
/// The construction is the whole of it: the `v` axis of a bare [`Uv`] read
/// through a [`CircleMap`] is that node's normalised radius, because the source
/// is addressed at `(angle, n)` and `n` runs from zero at the middle of the
/// window to one at its rim. What this graph adds is a contour. The window is
/// fixed at the widest disc the repeat holds, the radius is read back out of it
/// in UV, and `inner` and `radius` say where inside that disc the ramp starts
/// and where it has arrived. That is the arrangement [`rust`](super::rust)'s
/// `spread` uses over a distance plane, and for the same reason: a
/// [`CircleMap`]'s own radii are fields of the node, which nothing a caller
/// binds can reach, while a contour read off a fixed plane is a parameter that
/// really turns.
///
/// The ramp rises *outward*: zero at the middle of the repeat, one at
/// `radius`. So it is the falloff of a stain, the mask that keeps a
/// [`CircleSplatter`](crate::nodes::CircleSplatter) of bolts off the middle of
/// a plate, the vignette on a worn plate — and, through an [`Invert`], the
/// dome of a rivet head, which is the direction an author reaching for "a
/// gradient about a point" usually means. Turning it over is one node, and it
/// is left to the caller because a graph that exported the inverse would make
/// the half of the uses that want the falloff turn it over instead. The extra
/// it exports is called `gradient`, the same name [`angular_gradient`]
/// exports, so a graph can be handed either one without being rewired.
///
/// With `inner` above `radius` the ramp runs the other way — one at the middle,
/// zero past `inner` — which is the useful inverse rather than a mistake, and
/// with the two equal the division is by zero, which this crate answers with
/// zero: a flat field, which is what a ramp of no width should mean.
#[must_use]
pub fn radial_gradient() -> MaterialGraph {
    MaterialGraph::builder("patterns:radial_gradient")
        // Where the ramp leaves zero, as a radius in UV from the centre of the
        // repeat. Zero by default, so the default is the whole dome: an inner
        // radius is for a ring or a washer, and an author who wants one knows
        // it.
        .param(Param::float("inner", 0.0).range(0.0, WINDOW))
        // Where it arrives at one. The rim of the widest disc the repeat
        // holds, so that by default the gradient uses all the room there is
        // and the corners past it carry the one it ended on.
        .param(Param::float("radius", WINDOW).range(0.0, WINDOW))
        // The source: the `v` axis, which the circle map reads as its radial
        // coordinate. Nothing else of the source is read, so this is the
        // cheapest field that can be put through the node.
        .node("spoke", Decompose::new(Uv::new(), Channel::G))
        // The normalised radius, with a fill of one outside the window so the
        // corners carry the value the rim ended on rather than dropping back
        // to zero and drawing a ring nobody asked for.
        .node("disc", CircleMap::new("spoke").radius(WINDOW).outside(1.0))
        // Back into UV, which is the unit `inner` and `radius` are written in.
        .node("from_centre", Math::new(MathOp::Mul, "disc", WINDOW))
        .node(
            "span",
            Math::new(MathOp::Sub, Input::param("radius"), Input::param("inner")),
        )
        .node(
            "past_inner",
            Math::new(MathOp::Sub, "from_centre", Input::param("inner")),
        )
        .node("ramp", Math::new(MathOp::Div, "past_inner", "span"))
        .node("gradient", Clamp::new("ramp"))
        .output(
            PbrOutput::new()
                // A grey ramp: a float reaching a colour port broadcasts, so
                // the standalone bake is the field itself rather than a
                // picture somebody's palette decided.
                .base_color("gradient")
                .roughness(MATT)
                // The ramp read as a height, which is a dish rather than a
                // dome: it rises outward, so the middle of every repeat is the
                // lowest point of it and a rivet head is this height inverted.
                // Honest at the rim either way — the ramp reaches one exactly
                // there and the fill outside is the same one, so the surface
                // has no cliff at the window's edge for a derived normal to
                // find.
                .height("gradient")
                .normal_strength(RELIEF)
                .extra("gradient", "gradient"),
        )
        .into_graph()
}

/// A sweep round the centre of the repeat, from zero at angle zero to one a
/// turn later.
///
/// The same window as [`radial_gradient`] and the same node, reading the other
/// axis: a [`CircleMap`] addresses its source at `(fract(angle), n)`, so the
/// `u` axis of a bare [`Uv`] comes back as the angle in turns. What this adds
/// is the two things an author actually turns — where the sweep starts, and how
/// far it is carried round as it goes outward, which is what makes it a spiral.
/// Both are arithmetic on the field rather than the node's own `twist`, which
/// is a field of the node and so cannot be bound.
///
/// A sweep has one seam by construction: it wraps from one back to zero along
/// the ray at `offset`, which is where a clock's hand passes twelve. That is
/// what a sweep *is*, and it is why this binds no height — a step in a height
/// is a wall in the derived normal, and a wall that is an artefact of the
/// numbering rather than of the surface is the worst kind there is. Use the
/// field as a tint, a gate or a [`Warp`] offset, and where a surface really
/// does step, say so with a height of your own.
///
/// A dial, a fan, a brushed circular grind, the phase of a
/// [`CircleSplatter`](crate::nodes::CircleSplatter) round a flange.
#[must_use]
pub fn angular_gradient() -> MaterialGraph {
    MaterialGraph::builder("patterns:angular_gradient")
        // Which way the sweep starts, in turns. Zero puts its seam along the
        // +u axis, which is where the node's own angle begins; a quarter puts
        // it at the top, which is where a dial usually wants it.
        .param(Param::float("offset", 0.0).range(0.0, 1.0))
        // Turns of sweep carried per unit of radius, which is the spiral. Zero
        // by default because a spiral is a decision and a sweep is not: at
        // zero this is the plain angular ramp every use of it starts from.
        .param(Param::float("twist", 0.0).range(-4.0, 4.0))
        .node("sweep", Decompose::new(Uv::new(), Channel::R))
        .node("spoke", Decompose::new(Uv::new(), Channel::G))
        // The angle in turns inside the window, and zero outside it — the
        // value the sweep starts on, so the corners read as the beginning of
        // the sweep rather than as a fifth quadrant of their own.
        .node("around", CircleMap::new("sweep").radius(WINDOW))
        // The radius, for the twist to carry the angle by. One outside the
        // window, which keeps the corners on a single constant whatever the
        // twist is.
        .node(
            "across",
            CircleMap::new("spoke").radius(WINDOW).outside(1.0),
        )
        .node(
            "carried",
            Math::new(MathOp::Mul, "across", Input::param("twist")),
        )
        .node("spiralled", Math::new(MathOp::Add, "around", "carried"))
        .node(
            "shifted",
            Math::new(MathOp::Add, "spiralled", Input::param("offset")),
        )
        // The wrap, which is what keeps the sweep in `0..=1` once an offset and
        // a twist have been added to it.
        .node("gradient", Math::unary(MathOp::Fract, "shifted"))
        .output(
            PbrOutput::new()
                .base_color("gradient")
                .roughness(MATT)
                .extra("gradient", "gradient"),
        )
        .into_graph()
}

/// Growth rings across the repeat.
///
/// Twenty-four, which says what the repeat is: a board rather than a wall. At a
/// tile of about forty centimetres these rings stand a centimetre and a half
/// apart, which is a fast-grown softwood, and at 256 texels each ring is ten of
/// them — enough to carry a shape. The count is a field of the wave it is read
/// from and it has to be a whole number, or the field would not meet itself at
/// the seam, so it is a constant rather than a parameter: a ring count that
/// could be turned to 24.5 would be a plank with a fault down one edge.
const RINGS: u16 = 24;

/// The lattice the ring lines wander on, and its seed.
///
/// Four cells over the repeat and three octaves: what this displaces is the
/// whole ring pattern, and a board's rings bend over the length of the board
/// rather than over a centimetre. See `weathering::edge_wear`'s own break noise
/// for why a seed is a constant in this library.
const WANDER_PERIOD: u32 = 4;
const WANDER_SEED: u32 = 45;

/// The lattice the fibre is laid on, and its seed.
///
/// Four cells along the board and 256 across it. An anisotropic noise is how
/// this crate says "grain" — it is the construction the showcase's rolled metal
/// uses for its mill lines and the brick uses for its rain — and each cell of
/// one is a long thin streak lying the way a fibre lies, so displacing the
/// rings by it breaks every ring line into fibres rather than into blobs.
///
/// It is also the finest thing in this graph, so **a bake of wood wants 256
/// texels per repeat**. Coarser is refused by the lattice rule rather than
/// quietly filtered, which is the right answer: the fibre is most of what makes
/// the picture wood, and a wood without it is a stack of stripes.
const FIBRE_PERIOD: [u32; 2] = [4, 256];
const FIBRE_SEED: u32 = 46;

/// Where the two woods sit in the height, as a fraction of the unit.
///
/// Late wood is the dense band at the end of a season's growth, and it stands
/// proud of the early wood on any weathered board, because the early wood is
/// the softer of the two and goes first. Thirty hundredths between them leaves
/// room above and below for a caller to bed the board into a surface of their
/// own.
const EARLY_LEVEL: f32 = 0.35;
const LATE_LEVEL: f32 = 0.65;

/// What the two woods do to a finish.
///
/// Early wood is open pore and scatters; late wood is dense and takes a polish,
/// which is why a sanded board reads as striped under a grazing light even
/// where the two are nearly the same colour.
const EARLY_ROUGHNESS: f32 = 0.82;
const LATE_ROUGHNESS: f32 = 0.62;

/// Sawn timber: growth rings along one axis, bent by the log and broken into
/// fibre.
///
/// Three fields and no more. A sine along `v` lays `RINGS` growth rings
/// across the repeat; a coarse [`Noise`] pushes those lines about, which is the
/// difference between a printed stripe and a board cut out of a log that was
/// not a perfect cylinder; and an anisotropic noise, coarse along the board and
/// fine across it, pushes them again at the scale of a fibre. Both pushes are
/// one [`Warp`] rather than two, so the ring field is re-emitted once and the
/// two displacements are added before they are spent.
///
/// The wave is written out of the `v` axis rather than taken from a
/// [`Pattern`](crate::nodes::Pattern), which carries a wave on *both* axes and
/// combines them: a pattern of one sine along `v` still has a sine along `u`
/// multiplying it, and a board with a bright band down the middle of it is not
/// a board. The arithmetic here is the study concrete's own course arithmetic
/// with a sine where that has a fract.
///
/// `sharpness` is what separates a species from a stripe. The wave is
/// symmetric and no tree grows symmetrically: late wood is a narrow dark line
/// and early wood is most of the ring, so the field is raised to a power, which
/// narrows the dark band without moving where it is.
#[must_use]
pub fn wood() -> MaterialGraph {
    MaterialGraph::builder("patterns:wood")
        // How far the coarse noise carries a ring line, in UV. Six hundredths
        // of the repeat is a board off a log with some taper in it; zero is a
        // drawing of a board, and is what a test that wants to count rings
        // asks for.
        .param(Param::float("warp", 0.06).range(0.0, 0.5))
        // How far the fibre carries it, in the same units. Smaller by four
        // times, because a fibre is the width of a cell rather than the width
        // of a log: this is the term that makes the edge of a ring look torn
        // rather than drawn.
        .param(Param::float("fibre", 0.015).range(0.0, 0.2))
        // How narrow the late wood band is, as the exponent the ring field is
        // raised to. One is the bare sine, which reads as a woven stripe; a
        // little over two is a softwood, where the dark line is a fifth of the
        // ring and the rest is pale.
        .param(Param::float("sharpness", 2.2).range(0.2, 8.0))
        // Early wood: the pale, open half of a ring. A warm light brown in
        // linear terms, so the board reads as timber under a neutral light
        // without anybody grading it.
        .param(Param::color("early_color", [0.235, 0.140, 0.072]))
        // Late wood: the dense dark line. Browner and about a third as bright,
        // which is roughly what a pine board photographs at.
        .param(Param::color("late_color", [0.085, 0.043, 0.022]))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("growth", Math::new(MathOp::Mul, "v", f32::from(RINGS)))
        // `Sin` is in turns, so a whole number of rings meets itself at the
        // seam exactly, which is the whole reason the count is not a parameter.
        .node("wave", Math::unary(MathOp::Sin, "growth"))
        .node("rings_raw", Levels::new("wave").in_range(-1.0, 1.0))
        // Both displacements are signed: a noise in `0..=1` would only ever
        // push the rings one way, which is a translation of the board rather
        // than a bend in it.
        .node(
            "wander",
            Noise::perlin()
                .period(WANDER_PERIOD)
                .octaves(3)
                .persistence(0.55)
                .seed(WANDER_SEED),
        )
        .node("bend", Levels::new("wander").out_range(-1.0, 1.0))
        .node("bent", Math::new(MathOp::Mul, "bend", Input::param("warp")))
        .node(
            "grain_noise",
            Noise::value()
                .periods(FIBRE_PERIOD[0], FIBRE_PERIOD[1])
                .seed(FIBRE_SEED),
        )
        .node("fibres", Levels::new("grain_noise").out_range(-1.0, 1.0))
        .node(
            "torn",
            Math::new(MathOp::Mul, "fibres", Input::param("fibre")),
        )
        .node("displacement", Math::new(MathOp::Add, "bent", "torn"))
        // The amount is one and the parameters carry the distance, because a
        // `Warp`'s own amount is a field of the node: scaling the offset is how
        // a displacement becomes something a caller can turn. The ring field
        // depends on `v` alone, so displacing both axes by one number moves it
        // exactly as far as displacing `v` would.
        .node("warped", Warp::new("rings_raw", "displacement").amount(1.0))
        // The ring field proper: one in the late wood, zero in the early.
        .node(
            "rings",
            Math::new(MathOp::Pow, "warped", Input::param("sharpness")),
        )
        .node(
            "timber",
            Mix::new(
                Input::param("early_color"),
                Input::param("late_color"),
                "rings",
            ),
        )
        .node("finish", Mix::new(EARLY_ROUGHNESS, LATE_ROUGHNESS, "rings"))
        // Bedded by a `Levels` rather than added to anything, so the board's
        // surface is inside the unit by construction whatever the sharpness did
        // to the field.
        .node(
            "board",
            Levels::new("rings").out_range(EARLY_LEVEL, LATE_LEVEL),
        )
        .output(
            PbrOutput::new()
                .base_color("timber")
                .roughness("finish")
                .height("board")
                .normal_strength(RELIEF)
                // The deciding field, for a caller who wants the rings to
                // drive something of their own: a tint, a wear mask, the fibre
                // of a varnish.
                .extra("rings", "rings"),
        )
        .into_graph()
}

/// The cells the crack network is laid out on, and their seed.
///
/// Thirty-two plates over the repeat: at a metre and a half that is a plate a
/// little under five centimetres across, which is what a crazed render or a
/// dried bed of clay breaks into. A power of two, and that is the binding
/// constraint rather than the plate size: this graph exists to be instanced on
/// somebody else's surface, every lattice the crate lays is a power of two, and
/// a count that did not divide theirs would make the instanced repeat the least
/// common multiple of the two — a wall that tiled every 64 cells tiling every
/// 192 instead, with a warning to say so. The count is a field of the
/// [`Voronoi`] and so cannot be a parameter; what a caller turns instead is how
/// wide the cracks are and how far they wander off the lattice, which is what
/// actually reads.
const CELLS: u16 = 32;
const CELL_SEED: u32 = 47;

/// The lattice the crack lines wander on, and its seed.
///
/// Eight cells and two octaves, a quarter of the plate count: coarse enough
/// that a run of cracks leans one way over a stretch of wall, fine enough that
/// no single crack is a straight line. Eight divides the plate count, so the
/// graph's repeat is the plates' own and there is no least common multiple to
/// warn about.
const WANDER_CELLS: u32 = 8;
const CRACK_WANDER_SEED: u32 = 48;

/// The width of the ramp on a crack's shoulder, in cell units.
///
/// A little over a texel at 256 over [`CELLS`] plates. A crack has no soft
/// edge — it is a break, not a stain — so this is as narrow as it can be
/// without the line turning into a staircase, and it is a constant because a
/// softness nobody can see is not a decision anybody makes.
const CRACK_EDGE: f32 = 0.15;

/// The furthest the ground beside a crack is measured out to, in UV.
///
/// The [`Distance`] node's range, and so the ceiling on `chamfer`, for the same
/// reason `weathering::rust`'s reach is the ceiling on its spread: past it the
/// plane saturates and the slump would stop moving.
const CRACK_REACH: f32 = 0.05;

/// What counts as inside the crack mask when the distance plane is measured
/// from it.
const CRACK_THRESHOLD: f32 = 0.5;

/// Where the unbroken face stands in the unit.
///
/// Near the top, because everything this graph does to the height takes some of
/// it away: a face high in the unit leaves the whole of the rest for the crack
/// to cut into, and the clamp at the end catches a `depth` that asks for more
/// than there is.
const FACE_LEVEL: f32 = 0.9;

/// What a crack's inside does to a finish: it is a fracture, so it is matt, and
/// it holds whatever washed into it.
const CRACK_ROUGHNESS: f32 = 0.95;

/// A network of cracks over a field of plates.
///
/// [`Voronoi`] under [`VoronoiOutput::Edge`] answers the distance to the
/// boundary between the two nearest cells, and that is a crack network already:
/// every line meets two others at a point and no line ends in the middle of
/// nothing, which is what a break in a brittle surface does and what a
/// thresholded noise never manages. A [`Warp`] by a coarse noise takes the
/// lines off the lattice so they stop reading as a diagram, `width` reads the
/// line off the boundary field, and the input `mask` says where on the surface
/// any of this is allowed to happen at all.
///
/// The height is then measured rather than painted. A [`Distance`] plane from
/// the crack itself says how far each texel is from the nearest break and
/// `chamfer` reads the slump off it: the ground does not drop vertically at the
/// edge of a crack, it falls into it, and the width of that fall is the
/// difference between a crack in a render and a line drawn on one. The same
/// field darkens and roughens the surface, because the ground that slumped is
/// the ground that collects what washes in.
///
/// The crack mask is exported, which is half the point of the graph: a caller
/// cuts their own height with it, seeds a [`rust`](super::rust) along it or
/// gates a [`dirt_dust`](super::dirt_dust) by it, and gets a network that
/// agrees with this one to the texel rather than a second one that nearly does.
#[must_use]
pub fn cracks() -> MaterialGraph {
    let builder = MaterialGraph::builder("patterns:cracks")
        // How wide a crack is, in UV. Four thousandths of a metre-and-a-half
        // repeat is six millimetres: a break that has opened rather than a
        // hairline, which is the narrowest thing that still reads at the
        // resolutions a wall is baked at.
        .param(Param::float("width", 0.004).range(0.0, 0.02))
        // How far it cuts into the unit height. A third: deep enough that the
        // derived normal has an edge to catch the light on, shallow enough
        // that a caller can still bed this into a surface of their own.
        .param(Param::float("depth", 0.35).range(0.0, 1.0))
        // How far the ground slumps into the crack, in UV. A little over a
        // centimetre of the repeat, which is two or three times the crack's own
        // width — the proportion that reads as a break in something brittle
        // rather than as a cut in something soft.
        .param(Param::float("chamfer", 0.012).range(0.0, CRACK_REACH))
        // How far a crack line wanders off its lattice, in UV. Three
        // hundredths is most of a plate, which is enough that the cells stop
        // being visible as cells; zero is the bare lattice, and is what a test
        // that wants to know where the lines are asks for.
        .param(Param::float("warp", 0.03).range(0.0, 0.1))
        // The face between the cracks. A dry mineral grey — render, lime, a
        // dried bed — pale enough that the crack reads against it.
        .param(Param::color("face_color", [0.42, 0.40, 0.37]))
        // Inside the break. Nearly black, because what a crack shows is a gap
        // rather than a material, and a gap returns almost nothing.
        .param(Param::color("crack_color", [0.020, 0.018, 0.016]))
        // Where cracks are allowed at all: a caller's own field, a `WorldMask`
        // through the graph that instances this, or the constant one that
        // cracks the whole surface. One rather than zero for the reason
        // `stdlib::UNBIASED` gives.
        .node("mask", GraphInput::float("mask", UNBIASED))
        .node(
            "plates",
            Voronoi::new()
                .period(u32::from(CELLS))
                .seed(CELL_SEED)
                .output(VoronoiOutput::Edge),
        )
        .node(
            "wander",
            Noise::perlin()
                .period(WANDER_CELLS)
                .octaves(2)
                .persistence(0.55)
                .seed(CRACK_WANDER_SEED),
        )
        // Signed, so the lines wander both ways, and scaled by the parameter,
        // because a `Warp`'s amount is a field of the node.
        .node("drift", Levels::new("wander").out_range(-1.0, 1.0))
        .node(
            "displacement",
            Math::new(MathOp::Mul, "drift", Input::param("warp")),
        )
        .node("crazed", Warp::new("plates", "displacement").amount(1.0))
        // The boundary field is zero *on* the crack and grows into the plate,
        // so the line is what is left after it is turned over.
        .node("to_line", Invert::new("crazed"))
        // The width is authored in UV and read in cell units, because cell
        // units are what the boundary field answers in: a crack whose width
        // changed when the plate count did would be a parameter about the
        // lattice rather than about the surface.
        .node(
            "width_cells",
            Math::new(MathOp::Mul, Input::param("width"), f32::from(CELLS)),
        );
    // The shared threshold, whose `amount` is a contour on whatever field it is
    // handed: here the field is the distance *to* the crack line, so the
    // contour is the crack's own width and the ramp is its shoulder.
    let builder = thresholded(builder, "line", "to_line", "width_cells", CRACK_EDGE)
        // Gated before the distance plane is measured, so that a masked-off
        // stretch of wall has no crack for the slump to fall into either. With
        // nothing inside the mask at all the plane answers one everywhere, the
        // chamfer contour is negative, and the surface is the uncracked face.
        .node("crack_mask", Math::new(MathOp::Mul, "line", "mask"))
        .node(
            "from_crack",
            Distance::new("crack_mask")
                .threshold(CRACK_THRESHOLD)
                .range(CRACK_REACH),
        )
        .node("outward", Math::new(MathOp::Mul, "from_crack", CRACK_REACH))
        .node(
            "into_chamfer",
            Math::new(MathOp::Sub, Input::param("chamfer"), "outward"),
        )
        .node(
            "chamfer_ramp",
            Math::new(MathOp::Div, "into_chamfer", Input::param("chamfer")),
        )
        // One in the crack, falling to zero a chamfer away from it, and exactly
        // zero past that: everything below is a `Mix` or a subtraction by this,
        // so ground further from a crack than the chamfer reaches is the face
        // it started as, bit for bit.
        .node("slump", Clamp::new("chamfer_ramp"));
    builder
        .node(
            "sunk",
            Math::new(MathOp::Mul, "slump", Input::param("depth")),
        )
        .node("cut", Math::new(MathOp::Sub, FACE_LEVEL, "sunk"))
        // Held, because `depth` can ask for more than there is between the face
        // and the floor of the unit, and a height that leaves the unit is a
        // normal that is wrong exactly where it clipped.
        .node("crack_height", Clamp::new("cut"))
        .node(
            "surface",
            Mix::new(
                Input::param("face_color"),
                Input::param("crack_color"),
                "slump",
            ),
        )
        .node("finish", Mix::new(MATT, CRACK_ROUGHNESS, "slump"))
        .output(
            PbrOutput::new()
                .base_color("surface")
                .roughness("finish")
                .height("crack_height")
                .normal_strength(RELIEF)
                .extra("crack_mask", "crack_mask"),
        )
        .into_graph()
}
