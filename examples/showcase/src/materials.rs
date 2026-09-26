//! The showcase's own graphs: the four surfaces a texture cannot hold.
//!
//! Every surface a building here wears comes from the default library,
//! [`ashlar_material::stdlib`]. What is left to the showcase is what the
//! library cannot ship because it has to read the world: the light strip reads
//! the clock, the wet concrete and the weathered brick read the fragment's own
//! world normal and position, and the cut-aware concrete reads the mesh's cut
//! flag. Each instances a library graph through [`Subgraph`] and adds only
//! that one runtime input, so each ships as a compiled
//! [`ashlar::Surface::Shader`] beside the library's baked wall.
use ashlar_material::{
    Input, MaterialGraph, MaterialGraphBuilder, MaterialGraphLibrary, MathOp, Param, ParamValue,
    PbrOutput, SurfaceOutput,
    nodes::{Clamp, CutFlag, Invert, Levels, Math, Mix, Noise, Subgraph, Time, WorldMask},
};

/// The showcase's graph library: the default library and the four graphs
/// below, which instance it.
#[must_use]
pub fn graphs() -> MaterialGraphLibrary {
    let mut library = ashlar_material::stdlib::graphs();
    for graph in [
        concrete_wet(),
        concrete_cut_aware(),
        brick_weathered(),
        strip(),
    ] {
        library.insert(graph);
    }
    library
}

/// The showcase's own graphs, every one compiled per fragment and never baked.
#[must_use]
pub fn compiled() -> [&'static str; 4] {
    [
        "showcase:strip",
        "showcase:concrete-wet",
        "showcase:concrete-cut-aware",
        "showcase:brick-weathered",
    ]
}

/// What a soaked concrete face keeps of its dry albedo, and the roughness it
/// falls to.
///
/// Two numbers rather than a water shader. A film of water on a mineral surface
/// does two separable things: it fills the pores, so a good deal less diffuse
/// light escapes — a third is what a wet flagstone photographs at against its
/// dry half — and it is optically smooth, so the specular lobe collapses from
/// the 0.85-and-up of cast concrete to something near a varnish.
const CONCRETE_WET_ALBEDO: f32 = 0.20;

const CONCRETE_WET_ROUGHNESS: f32 = 0.10;

/// How wet the proud face is when the recesses are soaked.
///
/// Water runs downhill, and on a cast panel that means the pores, the pits and
/// the panel joint hold it while the face between them only damps. A single
/// `wetness` over the whole surface reads as somebody having turned the albedo
/// down; the same parameter weighted by the surface's *own height field* reads
/// as water, because the dark is then where a puddle would be. It costs one
/// invert, one remap and one multiply over the height texture the material
/// already binds, which is the cheapest thing in this graph and the one that
/// does the most for it.
///
/// A third rather than zero, because the high points of a wet slab are not dry.
/// They are wet and draining.
const CONCRETE_PROUD_WETNESS: f32 = 0.70;

/// How wet `showcase:concrete-wet` is where the rain reaches it.
///
/// High, and deliberately: the study is lit by two directional lights and no
/// environment map, so the thing that actually makes a wet surface read as wet
/// — what it reflects — is not available to it, and the only cue left is that
/// water darkens a mineral surface. At the design's illustrative 0.6 a coping
/// reads as damp beside the dry jamb next to it and as ordinary concrete on its
/// own; at this it reads wet in one look. It is a parameter, and a parameter is
/// a value somebody chooses.
pub const CONCRETE_WETNESS: f32 = 0.85;

/// How high above the ground rain still soaks a slab, in metres, and how wide
/// the ramp at that line is.
///
/// This is the half of the wetness gate that is about *where* a face is rather
/// than which way it points, and it is the one number here that is a heuristic
/// rather than a fact about water. What makes the coping of a middle storey dry
/// is that the storey above it covers it, and no fragment can ask whether
/// anything stands over it — so the material asks how high it is instead, and
/// six metres is above the study's one-storey roofline at 4.53 and well below
/// the 9.33 of the office's second band. A building whose first floor is its
/// last therefore stays wet across its whole roofline, and a stack dries out
/// above the first. The limit that leaves: the *lowest* band of a stack is
/// the same height as a low roofline, and a height cut-off cannot tell them
/// apart.
const RAIN_HEIGHT: f32 = 6.0;

const RAIN_SOFTNESS: f32 = 1.2;

/// The same cast concrete, wet where the weather reaches it.
///
/// A second graph rather than a parameter on `library:formed-concrete`: a bake
/// **refuses** a world-space input, because a texture has no axis to store a
/// fragment's own position or normal along. The library's concrete is baked, so
/// it cannot read either; this graph instances it — four
/// [`Subgraph`] nodes reading four of its outputs, inlined into one arena and
/// shared, so the whole of it is computed once — and adds what only a compiled
/// material can have.
///
/// What that buys is the debt phase three wrote down. There, which surfaces
/// were wet was a *slot*: a coping and a threshold, bound by hand, because a
/// material could not ask which way it faced. So the office, whose bays are
/// stacked, had a wet band across the middle of every storey — the coping of
/// the floor below, which the floor above covers. Here the material asks:
///
/// - **Which way the face points.** [`WorldMask::up`] is one on the faces rain
///   lands on and zero on the walls under them. The vertical front of a coping
///   — which is most of what a coping *shows* from the ground — is now as dry
///   as the wall it sits on, and its top is wet.
/// - **How high the face is.** [`RAIN_HEIGHT`] dries the bands above the first
///   storey, which is the office's own complaint, and leaves a one-storey
///   roofline and a doorway threshold exactly as wet as they were.
///
/// The two multiply, and the product scales the graph's own live `wetness`. So
/// a game can still turn the rain up and down with one uniform; what the world
/// decides is *where* that number lands.
fn concrete_wet() -> MaterialGraph {
    let builder = MaterialGraph::builder("showcase:concrete-wet")
        .param(
            Param::float("wetness", CONCRETE_WETNESS)
                .range(0.0, 1.0)
                .live(),
        )
        // The dry wall, four outputs of it, instanced and inlined. A subgraph's
        // parameters are bound at the instance and folded, so the `wetness`
        // inside this one sits at its own default of zero and the mixes it
        // carries are the identity — the surface that arrives here is the
        // surface the baked wall is.
        .node(
            "albedo",
            Subgraph::new("library:formed-concrete").output(SurfaceOutput::BaseColor),
        )
        .node(
            "roughness",
            Subgraph::new("library:formed-concrete").output(SurfaceOutput::Roughness),
        )
        .node(
            "height",
            Subgraph::new("library:formed-concrete").output(SurfaceOutput::Height),
        )
        .node(
            "occlusion",
            Subgraph::new("library:formed-concrete").output(SurfaceOutput::Occlusion),
        )
        .node("up_facing", WorldMask::up())
        .node(
            "near_ground",
            WorldMask::below(RAIN_HEIGHT).softness(RAIN_SOFTNESS),
        )
        .node(
            "rained_on",
            Math::new(MathOp::Mul, "up_facing", "near_ground"),
        )
        .node(
            "falling",
            Math::new(MathOp::Mul, "rained_on", Input::param("wetness")),
        );
    wetted(builder, "falling")
        .output(
            PbrOutput::new()
                .base_color("wet_face")
                .roughness("wet_gloss")
                .metallic(0.0)
                .occlusion("occlusion")
                .height("height")
                .normal_strength(0.01),
        )
        .tile_metres([2.0; 2])
        .into_graph()
}

/// How much darker a saw-cut face of the study's concrete is than its formed
/// face, and how much rougher.
///
/// The tone is the number the study's old `study:cut` material carried,
/// read off its own constants rather than invented: that slot multiplies the
/// same maps by `(0.34, 0.35, 0.36)` where the wall multiplies them by
/// `(0.36, 0.39, 0.40)`, which is 0.944, 0.897 and 0.900 of the wall — nine
/// tenths, to the precision anybody chose it at. A material that darkens its own
/// cut faces should land where the slot that did it by hand landed, or the
/// comparison in the gallery is between two treatments rather than between two
/// ways of delivering one.
///
/// The roughness has no such precedent: `study:cut` and the old concrete carried
/// the same 1.0 factor, because a *constant* cannot make one face of one mesh
/// rougher than another and the slot only ever moved the colour. This is the
/// thing the slot could have done and did not. A cut face is darker because it
/// is the inside of the slab rather than the skin the formwork pressed, and
/// rougher for the same reason: the fines a mould draws to the surface are what
/// makes a formed face smooth, and a saw does not. Eight hundredths over a
/// graph that already answers 0.85 to 0.97 is most of the way to the 1.0 the
/// clamp stops at.
const CUT_TONE: f32 = 0.9;

const CUT_ROUGHNESS: f32 = 0.08;

/// The same cast concrete, dark and rough where a cutter made the face.
///
/// The debt the `cut_material_slot` was: a reveal, a doorhead and a sawn return
/// are the *same material* as the wall around them, worn differently, and until
/// the mesh could say which face was which the only way to draw them differently
/// was a second material key and a second entry in every palette. The kernel has
/// always known — `ashlar::TriangleMesh::cut_faces` answers for one triangle at
/// a time and `ashlar-manifold` fills in each triangle's source — and
/// `ashlar-bevy` now carries that flag to the
/// fragment as a vertex attribute, so a material can ask.
///
/// It asks the same way [`concrete_wet`] asks which way it faces: four
/// [`Subgraph`] nodes on `library:formed-concrete`, inlined into one arena and shared, so
/// the whole of the wall is computed once; then two [`Mix`] nodes weighted by
/// [`CutFlag`], darkening the albedo and roughening the gloss. Nothing else
/// moves — the height, the normal derived from it and the occlusion are the
/// wall's own, because a saw cut through cast concrete has the same aggregate
/// in it as the slab it went through.
///
/// A `Mix` and not a multiply, so that the uncut face is *exactly* the wall:
/// `Op::Mix` is `a + (b - a) * t` and at `t = 0` that is `a + 0`, bit for bit.
/// One material, one mesh, one draw, and the slot stays supported for the
/// surfaces that really are a different material.
///
/// Nothing live. The two numbers above are decisions about what cut concrete
/// *is*, made when the material is authored, so they fold; what the compiled
/// half carries is the flag and the nine instructions downstream of it, over
/// seven bound textures and no uniform at all. It is the first graph in the
/// study whose runtime half is about neither the clock nor a slider.
fn concrete_cut_aware() -> MaterialGraph {
    MaterialGraph::builder("showcase:concrete-cut-aware")
        .node(
            "albedo",
            Subgraph::new("library:formed-concrete").output(SurfaceOutput::BaseColor),
        )
        .node(
            "roughness",
            Subgraph::new("library:formed-concrete").output(SurfaceOutput::Roughness),
        )
        .node(
            "height",
            Subgraph::new("library:formed-concrete").output(SurfaceOutput::Height),
        )
        .node(
            "occlusion",
            Subgraph::new("library:formed-concrete").output(SurfaceOutput::Occlusion),
        )
        .node("sawn", CutFlag::new())
        .node("aggregate", Math::new(MathOp::Mul, "albedo", CUT_TONE))
        .node("face", Mix::new("albedo", "aggregate", "sawn"))
        .node("coarse", Math::new(MathOp::Add, "roughness", CUT_ROUGHNESS))
        .node("dulled", Clamp::new("coarse"))
        .node("gloss", Mix::new("roughness", "dulled", "sawn"))
        .output(
            PbrOutput::new()
                .base_color("face")
                .roughness("gloss")
                .metallic(0.0)
                .occlusion("occlusion")
                .height("height")
                .normal_strength(0.01),
        )
        .tile_metres([2.0; 2])
        .into_graph()
}

/// Append the six nodes that let water collect on the concrete, and leave
/// `wet_face` and `wet_gloss` where the output can bind them.
///
/// `wetness` is how wet the slab is as a whole, gated by where the fragment is in
/// [`concrete_wet`]. The whole of what it costs is two lerps over maps the
/// material binds anyway, weighted by a third lerp against the height.
/// Everything it touches is reached by [`Mix`], which is what makes the dry
/// wall *exactly* the wall it was — `Op::Mix` is `a + (b - a) * t`, and at
/// `t = 0` that is `a + 0`, bit for bit, for every finite `a`.
fn wetted(builder: MaterialGraphBuilder, wetness: impl Into<Input>) -> MaterialGraphBuilder {
    builder
        // Where the water is: the height field upside down, lifted so the proud
        // face damps rather than dries, and scaled by how wet the slab is.
        .node("pooled", Invert::new("height"))
        .node(
            "held",
            Levels::new("pooled").out_range(CONCRETE_PROUD_WETNESS, 1.0),
        )
        .node("soak", Math::new(MathOp::Mul, "held", wetness))
        .node(
            "soaked",
            Math::new(MathOp::Mul, "albedo", CONCRETE_WET_ALBEDO),
        )
        .node("wet_face", Mix::new("albedo", "soaked", "soak"))
        .node(
            "wet_gloss",
            Mix::new("roughness", CONCRETE_WET_ROUGHNESS, "soak"),
        )
}

/// Half the width of the world gate's ramp, in the cosine it is a threshold on.
///
/// [`WorldMask::up`] is a half at sixty degrees off vertical, and this is how
/// far either side of that the answer takes to travel. Wider than the node's own
/// quarter, and the reason is the mesh rather than the weather: the study's
/// solids are chamfered and its mesher creases every edge, so the mask is
/// *constant across a facet* and what an author is choosing here is not a
/// gradient but which facets land in the middle. At 0.35 the ramp runs from 0.15
/// to 0.85 of the cosine and the ramp between them is a smoothstep, which puts a
/// forty-five degree chamfer — every arris of every coping, sill and plinth in
/// the study, at a cosine of 0.707 — at about 0.89 rather than hard against the
/// horizontal face above it or the wall below it. That is what
/// reads as a wash fading off a return; a knife edge would make each chamfer
/// wholly filthy or wholly clean and draw the soot's boundary on a line nobody
/// put there.
const DUST_FALLOFF: f32 = 0.35;

/// The colour of what settles on a building in a street.
///
/// The one thing this graph overrules `weathering:dirt_dust` about, and it is
/// overruled in the direction nobody expects: the compound's own warm pale grey
/// is *lighter* than this wall, because interior dust is, and laying it on a
/// brick elevation would read as the wall having been rendered rather than as
/// the wall being dirty. What falls on the outside of a building is soot and
/// traffic film. At a luminance near 0.05 it sits between the brick's own grime
/// — `library:brick` mixes 0.03 into its hollows — and its mortar, which is where
/// three decades of a main road actually leaves a parapet.
const WALL_SOOT: [f32; 3] = [0.055, 0.052, 0.048];

/// How much of the wall is pore rather than solid.
///
/// The other thing this graph overrules, and a fact about the material rather
/// than a taste: `weathering:moisture` defaults to a half, which is cast
/// concrete, and fired clay laid in lime is well past that — a brick takes up a
/// fifth of its own weight in water, and a masonry wall *stays* dark for an hour
/// after the rain has stopped while the painted steel beside it is dry in
/// minutes. Three quarters puts the damp face at seven tenths of its dry
/// roughness: measured, 0.754 dry against 0.528 wet, with the joints at the
/// water's own 0.02.
const WALL_POROSITY: f32 = 0.75;

/// The library brick on a real elevation: soot where the world lets it settle,
/// and rain a game can turn on.
///
/// The world-bias idiom, on the wall it was written for. Nothing here is a new
/// field. `library:brick` is instanced whole through four [`Subgraph`] nodes, and the
/// two things that happen to a wall standing in a street are
/// [`ashlar_material::stdlib`] compounds laid over it with
/// [`MaterialGraphBuilder::layer`]. What this graph decides is the two *gates*,
/// and they are deliberately the two different kinds a compound's bias can be:
///
/// - **`weathering:dirt_dust` is gated by the world.** Its `bias` is
///   [`WorldMask::up`], so soot settles on the faces that point up — a coping, a
///   sill, the top of a plinth — and the elevation between them stays the wall
///   it was. That is a question no texture can answer, because the same map is
///   on every face of the mesh, and it is why this graph is compiled.
/// - **`weathering:moisture` is gated by a live parameter.** Its `wet` input is
///   this graph's own `wet`, declared `.live()`, so a game moves the rain with
///   one uniform. `moisture` is the compound built for that: every filter in it
///   is pointwise, so nothing it touches has to become a plane and nothing
///   freezes.
///
/// The order is the order things happen in. Dust settles on a dry wall and the
/// rain then runs over it, darkening the soot along with everything else, which
/// is why the moisture layer reads the dirt's colour rather than the brick's.
///
/// Almost nothing else is said. `amount`, `softness`, `level`, `darkening` — the
/// four numbers the two compounds decide the picture with — are left exactly
/// where the standard library put them, and that is worth one sentence: the
/// compounds' defaults assume a relief of a centimetre over a metre-and-a-half
/// repeat, and [`BRICK_RELIEF`] is nine millimetres over 1.72, so this is the
/// surface they were calibrated against. [`rusted_steel`] had to run its
/// `amount` up to 0.96 because rolled sheet is a tenth as deep as this; a raked
/// bed joint is precisely what a shelter filter is looking for. The two
/// overrides are [`WALL_SOOT`] and [`WALL_POROSITY`], and both are facts about
/// brick rather than adjustments to a picture.
///
/// # Why the height is the brick's and not the water's
///
/// `weathering:moisture` offers to raise the height to its waterline where the
/// surface is submerged, which flattens the derived normal across a puddle — and
/// this graph does not take it. A puddle is a thing a *floor* has. This wall
/// stands up: water on it clings and runs, a bed joint eleven millimetres wide
/// holds a film rather than a pool, and flattening the joint's own relief under
/// that film would take away the shadow line the wall is read by.
///
/// Taking it would also cost three quarters again. A height that depends on the
/// live uniform is a normal the fragment has to finite-difference for itself,
/// and the two together are the difference between the numbers below and
/// *ninety* ops and four neighbourhood taps over *eight* bound images — all
/// eight of the pairs one Bevy material extension declares — with
/// `CostReport::widens` false. False there means the roughness keeps its
/// level-zero value at every distance, and on a wall whose wet joints sit at a
/// roughness of 0.02 that is a glitter which outstays its detail. Binding the
/// brick's own height leaves the derived normal a static plane, so the packer
/// folds the scalars four to an image and the roughness widens with the mip
/// chain the way the baked twin's does.
///
/// # What it costs
///
/// At 1024 with `wet` live, which is
/// `partition(&material, &Target::shader_for(&material), 1024)`:
///
/// ```text
/// live: 1 uniform(s), 51 ops and 10 texture reads per fragment over 5 bound texture(s)
///   51 ops and 10 texture reads per fragment, 1 uniform(s)
///   9 bound value(s) in 5 texture(s), from 19 plane(s)
///   base_color: 36 ops
///   roughness: 27 ops
/// ```
///
/// The shape of that is the point. Nineteen planes are rasterised, nine of their
/// values are uploaded, and the packer folds those nine into five images — the
/// bond, the clay, the chips, the dust the joints shed, the occlusion march, and
/// the whole of `weathering:dirt_dust` including its own horizon march and its
/// curvature — because all of it is dry arithmetic over the coordinate. What is left in the fragment is `weathering:moisture`
/// and the [`WorldMask`] the dirt's bias could not fold. `frozen` is empty: no
/// live value reaches a filter that has to be a plane, which is the property
/// `moisture` was built to have.
///
/// # No texture set
///
/// It reads the fragment's own world normal, so a bake refuses it by path rather
/// than folding it to zero, exactly as [`concrete_wet`] is refused — *"a world
/// normal is a fact about the mesh and the frame it is drawn in, and a texture
/// has no axis to store one along"*. It is in [`compiled`], and what pins it is that it partitions and
/// that the fragment agrees with the interpreter.
///
/// # Metalness and occlusion
///
/// Bound by the caller rather than read off a compound, for two different
/// reasons. Fired clay is a dielectric at every state of wetness, so the
/// metalness is the constant zero `library:brick` itself writes; reading `moisture`'s,
/// which mixes a zero towards a zero under the puddle, would cost a plane to say
/// nothing. The occlusion is the brick's own horizon march, because neither
/// compound changes the *shape* of the wall — and binding it is what lets the
/// packer put nine scalars in five images rather than eight in eight.
fn brick_weathered() -> MaterialGraph {
    MaterialGraph::builder("showcase:brick-weathered")
        // The one live parameter: how much rain has arrived. Zero by default, so
        // a game that loads this and touches nothing gets the dry wall — and,
        // because everything `moisture` does is reached through a `Mix`, it is
        // the dry wall exactly rather than nearly.
        .param(Param::float("wet", 0.0).range(0.0, 1.0).live())
        // The wall: one instance of the library brick, four ports.
        .layer(
            "brick",
            Subgraph::new("library:brick"),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Occlusion,
                SurfaceOutput::Height,
            ],
        )
        // Which way this fragment's face points, which is the half of the gate a
        // texture cannot hold.
        .node("upward", WorldMask::up().softness(DUST_FALLOFF))
        .layer(
            "dirt",
            Subgraph::new("weathering:dirt_dust")
                .input("height", "brick.height")
                .input("base_color", "brick.base_color")
                .input("roughness", "brick.roughness")
                .input("bias", "upward")
                // The wall's own relief, so the compound's two filters read the
                // height as the length it is. Only the caller knows this number,
                // which is why it is a parameter rather than a constant.
                .param("relief", ParamValue::Float(0.02))
                .param("color", ParamValue::Color(WALL_SOOT)),
            &[SurfaceOutput::BaseColor, SurfaceOutput::Roughness],
        )
        .layer(
            "wet",
            Subgraph::new("weathering:moisture")
                .input("height", "brick.height")
                .input("base_color", "dirt.base_color")
                .input("roughness", "dirt.roughness")
                .input("wet", Input::param("wet"))
                .param("porosity", ParamValue::Float(WALL_POROSITY)),
            &[SurfaceOutput::BaseColor, SurfaceOutput::Roughness],
        )
        .output(
            PbrOutput::new()
                .base_color("wet.base_color")
                .roughness("wet.roughness")
                .metallic(0.0)
                .occlusion("brick.occlusion")
                .height("brick.height")
                .normal_strength(0.02),
        )
        .tile_metres([2.0; 2])
        .into_graph()
}

/// Turns a second the strip's pulse runs at, by default.
///
/// [`MathOp::Sin`] takes turns rather than radians, so this is directly a
/// frequency: a little over a third of a turn a second is a cycle of just under
/// three seconds, which reads as a fluorescent tube's ballast hum rather than
/// as a strobe and is slow enough that two screenshots a second apart are
/// visibly different brightnesses. It is a `Bake` parameter on purpose — see
/// [`strip`].
const STRIP_RATE: f32 = 0.35;

/// How far either side of three quarters the pulse swings.
///
/// The design's figure: `0.75 + 0.25 * sin(...)`, so the tube runs between half
/// and full and never goes dark. A strip that reached zero would read as a
/// fault rather than as a light.
const STRIP_FLOOR: f32 = 0.75;

const STRIP_SWING: f32 = 0.25;

/// How much of a tube one repeat of the strip covers.
///
/// The one shipped surface whose repeat is not square, and the reason it is a
/// constant rather than a literal in two files: the graph lays its grain along
/// a two and a quarter metre tube and across a strip half a metre wide, and the
/// definition in [`crate::library`] has to lay it over exactly that or the
/// grain runs eight times over the tube instead of once.
pub const STRIP_TILE_METRES: [f32; 2] = [2.25, 0.5];

/// The study's light strip: an extruded diffuser with the clock behind it.
///
/// The first graph in this workspace that a texture cannot hold. Everything
/// else here is a field over UV, and a field over UV is what a bake is; a pulse
/// is a field over *time*, and an image has no axis to store it along. So this
/// one ships as a [`ashlar::Surface::Shader`] and is compiled rather than
/// baked — which is the whole of phase three in one material.
///
/// What it costs is the point. The diffuser is static: its grain, its dust and
/// its roughness are baked into bound textures exactly as they would be for any
/// other surface, and the fragment does not evaluate a noise. What is left on
/// the runtime side is the clock, one multiply into a frequency, a sine, a
/// scale, an add and a multiply into the emissive texture — a handful of
/// instructions, and the cost report prints them when the preview loads it.
///
/// # Why `rate` is a `Bake` parameter and the pulse is still live
///
/// The two are different kinds of thing, and the distinction is the one the
/// partition exists to draw. `rate` is a decision about what this light *is*,
/// made once when the material is authored; folding it costs nothing and
/// leaves one fewer uniform. The *phase* is not a decision at all — it is the
/// clock, and the clock is a runtime input rather than a parameter. A live
/// `rate` would let a game change the frequency per frame, which nothing wants
/// and which would keep the multiply alive for no one.
///
/// # No height
///
/// The one shipped graph that binds none, and deliberately: the strip is
/// forty-five millimetres of flat acrylic, a normal map over it would describe
/// relief nobody can see at that size, and every map a graph does not bind is a
/// texture the compiled material does not bind either. `PbrOutput` derives its
/// normal from height, so binding no height is also what keeps the partition's
/// derived-normal root out of this one.
fn strip() -> MaterialGraph {
    MaterialGraph::builder("showcase:strip")
        .param(Param::float("rate", STRIP_RATE).range(0.0, 2.0))
        // The diffuser: extruded acrylic, so the grain runs along the tube, and
        // the dust that has settled on it does not.
        .node("grain", Noise::value().periods(4, 64).seed(51))
        .node("dust_field", Noise::value().period(128).seed(52))
        .node("body", Levels::new("grain").out_range(0.90, 1.00))
        .node("dust", Levels::new("dust_field").out_range(0.94, 1.02))
        .node("diffuser", Math::new(MathOp::Mul, "body", "dust"))
        // The clock, and the only thing in the graph that moves.
        .node("clock", Time::new())
        .node(
            "phase",
            Math::new(MathOp::Mul, "clock", Input::param("rate")),
        )
        .node("wave", Math::unary(MathOp::Sin, "phase"))
        .node("swing", Math::new(MathOp::Mul, "wave", STRIP_SWING))
        .node("pulse", Math::new(MathOp::Add, STRIP_FLOOR, "swing"))
        .node("emissive", Math::new(MathOp::Mul, "diffuser", "pulse"))
        // A diffuser is a satin surface, a shade duller where the dust is.
        .node("roughness", Levels::new("grain").out_range(0.30, 0.42))
        .output(
            PbrOutput::new()
                // Near white, because the definition's own constants are what
                // colour this light; the graph describes the acrylic.
                .base_color("diffuser")
                .roughness("roughness")
                .metallic(0.0)
                .occlusion(1.0)
                .emissive("emissive"),
        )
        .tile_metres(STRIP_TILE_METRES)
        .into_graph()
}
