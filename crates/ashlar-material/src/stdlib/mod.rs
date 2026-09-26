//! The standard library: the default `library:*` surfaces, and the layouts,
//! substances, weathering compounds and pattern recipes they are built from.
//!
//! These are library graphs rather than nodes. A wall that has been rained on
//! for thirty years is not a primitive — it is a dozen of them wired the way
//! the study graphs wire theirs — and the thing worth shipping is the wiring.
//! So each entry here is a [`MaterialGraph`](crate::MaterialGraph) with typed inputs, exposed
//! parameters and a named mask, built by a Rust function, and
//! [`graphs`] answers the whole set as a [`MaterialGraphLibrary`] a game
//! merges into its own with [`MaterialGraphLibrary::extend`].
//!
//! They live in the crate rather than in the showcase for two reasons. The
//! crate's own tests hold them to the tiling, lattice, lowering and WGSL rules
//! every graph here lives under, which is the whole of what makes a compound
//! safe to instance; and a game that wants dirt in its hollows should get it
//! by depending on this crate rather than by copying six hundred lines of
//! builder out of an example.
//!
//! # The convention a compound follows
//!
//! - **Inputs are the substrate's channels it changes**, plus whatever bias
//!   the caller wants to gate it with, each a [`GraphInput`]
//!   whose default makes the graph bake to a plausible picture on its own: a
//!   flat height at `FLAT_HEIGHT`, a mid grey at `MID_GREY`, a matt
//!   `MATT` roughness. That is what lets a compound be built, baked,
//!   previewed and tested standalone, and it is why the tests in
//!   `tests/stdlib.rs` can bake every one of them against no caller at all.
//! - **Parameters are the author's decisions**, and they fold: a
//!   [`Subgraph`](crate::nodes::Subgraph) binds them at the instance and the
//!   lowering turns them into constants.
//! - **Outputs are the channels it changes and no others.** A compound that
//!   does not touch the height binds none, so a caller that reads one gets the
//!   resolver's refusal rather than a silent passthrough of a default it never
//!   asked for. The four channels [`PbrOutput`](crate::PbrOutput) binds
//!   unconditionally are the exception the type forces: a compound that does
//!   not change the metalness leaves it on the output's own default, and a
//!   caller should read the channels the compound's documentation lists and
//!   nothing else.
//! - **Every compound exports its deciding mask** through
//!   [`PbrOutput::extra`](crate::PbrOutput::extra), so a caller can layer on
//!   the same decision — dirt in the same hollows, rust at the same edge —
//!   rather than build a second mask that agrees with the first to within a
//!   texel.
//! - **No compound reads a runtime input.** A bake refuses
//!   [`WorldMask`](crate::nodes::WorldMask) and its relatives by path, so a
//!   compound that read one inside itself could never be baked or tested
//!   alone. World-space bias belongs to the caller: the compound takes a plain
//!   float `bias` input and the graph that instances it wires
//!   `WorldMask::up()`, a live parameter or a constant into it.
//!
//! # A pattern is not a compound
//!
//! The `patterns:*` half of the library draws a field rather than changing one:
//! a gradient, a board, a crack network. So a pattern declares no channel
//! inputs — there is no substrate to take — and what it binds is whatever makes
//! its own bake a picture of itself rather than only the channels a caller
//! would layer. What it shares with a compound is the part that matters: the
//! field it was built for is exported as an extra, because a caller who
//! instances `patterns:cracks` almost always wants the crack mask to cut their
//! own surface with rather than the grey render this bakes on its own.
//!
//! # Two things a parameter cannot be
//!
//! A [`Param`](crate::Param) reaches input ports, and a good deal of what an
//! author would like to turn is not one. A filter radius, a noise seed, an
//! octave count and a generator's period are *fields* of their node rather
//! than ports — there is nowhere to wire a value into them — so where the
//! design called for a parameter that cannot be wired, the number is a
//! documented constant in the builder instead, and the doc comment says what
//! it is and why it is that. Exposing a parameter that reaches nothing would
//! be worse than not exposing it: it would validate, bind, and quietly do
//! nothing.
//!
//! # Relief
//!
//! Every compound that reads a *neighbourhood* of the height — an occlusion,
//! a curvature — takes a `relief` parameter, because those filters read the
//! height as a length in the same units as UV while a height input is a
//! `0..=1` field. It is the same number the caller puts in
//! [`PbrOutput::normal_strength`](crate::PbrOutput::normal_strength), and
//! multiplying by it is what `relief_of` does. What it buys beyond
//! correctness is that a compound instanced onto a shallow surface wears less
//! than the same compound on a deep one, which is what a real edge does.

mod ceramics;
mod city;
mod ground;
mod layouts;
mod metals;
mod moss;
mod patterns;
mod reference_brick;
mod reference_carpet;
mod reference_cobblestone;
mod reference_grass;
mod reference_moss;
mod reference_support;
mod render;
mod scifi;
mod surfaces;
mod timber;
mod utility;
mod weathering;

pub use ceramics::{ceramic_tile, clay_roof_tiles, fired_clay, lapped_courses};
pub use city::{
    curtain_wall, holo_sign, interior_panelling, office_window, road, shopfront, stained_concrete,
    window_band,
};
pub use ground::{asphalt, bitumen_aggregate, rubble};
pub use layouts::slab_lattice;
pub use metals::{
    corrugated_steel, corrugation, paint_film, painted_metal, rusted_steel, steel, steel_substance,
};
pub use moss::moss;
pub use patterns::{angular_gradient, cracks, radial_gradient, wood};
pub use reference_brick::brick;
pub use reference_carpet::{SHOOTS_PER_CELL as CARPET_SHOOTS_PER_CELL, moss_carpet};
pub use reference_cobblestone::cobblestone;
pub use reference_grass::grass;
pub use reference_moss::{brick_moss, cobblestone_moss};
pub use render::{damaged_plaster, lime_render, plaster};
pub use scifi::{adobe, desert_sand, hull_plating, tread_plate};
pub use surfaces::{
    ashlar_blocks, cast_cement, cut_limestone, formed_concrete, paving_slabs, stone_cladding,
};
pub use timber::{board_lattice, painted_boards, timber, wood_floor};
pub use utility::{dark_recess, emissive_strip, glass, signage_ink};
pub use weathering::{dirt_dust, edge_wear, moisture, peeling_paint, rust, streaks};

use crate::{
    Input, MaterialGraphBuilder, MaterialGraphLibrary, MathOp,
    nodes::{Clamp, GraphInput, Math},
};

/// Every graph this crate ships, keyed `library:<name>` for a finished
/// surface and `layouts:`, `substances:`, `weathering:` or `patterns:` for the
/// parts they are built from.
///
/// The keys are the whole of the interface: a game merges this into its own
/// library with [`MaterialGraphLibrary::extend`], which refuses a collision
/// rather than replacing either side, and its graphs then name these keys from
/// [`Subgraph`](crate::nodes::Subgraph) nodes.
///
/// ```
/// use ashlar_material::{MaterialGraphLibrary, stdlib};
///
/// let library = stdlib::graphs();
/// assert!(library.get("weathering:edge_wear").is_some());
/// // Every one of them validates on its own, with nothing wired in.
/// library.check()?;
/// # Ok::<(), ashlar_material::GraphError>(())
/// ```
#[must_use]
pub fn graphs() -> MaterialGraphLibrary {
    let mut library = MaterialGraphLibrary::default();
    for graph in vec![
        edge_wear(),
        dirt_dust(),
        rust(),
        peeling_paint(),
        moisture(),
        streaks(),
        moss(),
        radial_gradient(),
        angular_gradient(),
        wood(),
        cracks(),
        cast_cement(),
        formed_concrete(),
        slab_lattice(),
        paving_slabs(),
        cut_limestone(),
        stone_cladding(),
        ashlar_blocks(),
        brick(17),
        lime_render(),
        plaster(),
        damaged_plaster(),
        steel_substance(),
        steel(),
        paint_film(),
        painted_metal(),
        rusted_steel(),
        corrugation(),
        corrugated_steel(),
        board_lattice(),
        timber(),
        wood_floor(),
        painted_boards(),
        fired_clay(),
        lapped_courses(),
        clay_roof_tiles(),
        ceramic_tile(),
        bitumen_aggregate(),
        asphalt(),
        rubble(),
        glass(),
        dark_recess(),
        emissive_strip(),
        signage_ink(),
        hull_plating(),
        tread_plate(),
        adobe(),
        desert_sand(),
        curtain_wall(),
        window_band(),
        road(),
        interior_panelling(),
        holo_sign(),
        stained_concrete(),
        office_window(),
        shopfront(),
        cobblestone(17),
        grass(17),
        moss_carpet(17),
        brick_moss(false),
        brick_moss(true),
        cobblestone_moss(false),
        cobblestone_moss(true),
    ] {
        library.insert(graph);
    }
    library
}

/// The radiance a default definition multiplies its graph's emissive map by:
/// nonzero only for the surfaces that emit.
fn emissive(key: &str) -> [f32; 3] {
    match key {
        "library:emissive-strip" => [2.0, 3.5, 4.0],
        "library:curtain-wall" | "library:window-band" => [14.0; 3],
        "library:interior-panelling" => [40.0; 3],
        "library:holo-sign" => [22.0; 3],
        "library:office-window" => [1.0, 0.8, 0.55],
        "library:shopfront" => [1.0; 3],
        _ => [0.0; 3],
    }
}

/// Default building materials, with their graph's physical repeat and a
/// bounded 512-square runtime bake. Merge [`graphs`] into the graph library
/// supplied to the renderer alongside these definitions.
#[must_use]
pub fn materials() -> ashlar_surface::MaterialLibrary {
    use ashlar_surface::{Bake, MaterialDefinition, MaterialLibrary, Surface};
    let graphs = graphs();
    MaterialLibrary {
        materials: graphs
            .graphs
            .iter()
            .filter(|(key, _)| key.starts_with("library:"))
            .map(|(key, graph)| {
                (
                    key.clone(),
                    MaterialDefinition {
                        tile_metres: graph.tile_metres.unwrap_or([2.0; 2]),
                        emissive: emissive(key),
                        roughness: 1.0,
                        metallic: 1.0,
                        strands: match key.as_str() {
                            "library:grass" => Some(
                                ashlar_surface::StrandSettings::new([
                                    "blades",
                                    "fibres",
                                    "stragglers",
                                ])
                                .lod_metres([2.5, 7.0])
                                .card_metres(14.0)
                                .density(1.0)
                                .cast_shadows(false),
                            ),
                            "library:moss-carpet" => Some(
                                ashlar_surface::StrandSettings::new(["shoots"])
                                    .lod_metres([2.5, 7.0])
                                    .density(1.0)
                                    .cast_shadows(false),
                            ),
                            _ => None,
                        },
                        surface: Surface::Graph(Bake {
                            graph: key.clone(),
                            params: graph
                                .params
                                .iter()
                                .map(|p| (p.name.clone(), p.value))
                                .collect(),
                            resolution: 512,
                        }),
                        ..MaterialDefinition::default()
                    },
                )
            })
            .collect(),
    }
}

/// What a height input carries with nothing bound to it: a flat field halfway
/// up the unit interval, which leaves room to be cut into and built onto.
pub(crate) const FLAT_HEIGHT: f32 = 0.5;

/// What a colour input carries with nothing bound to it.
///
/// A mid grey rather than white, so that a compound which darkens and one
/// which lightens both read on the standalone bake.
pub(crate) const MID_GREY: [f32; 3] = [0.5, 0.5, 0.5];

/// What a roughness input carries with nothing bound to it: a matt dielectric,
/// which is what most of a building is.
pub(crate) const MATT: f32 = 0.8;

/// What a metalness input carries with nothing bound to it.
pub(crate) const DIELECTRIC: f32 = 0.0;

/// What a bias input carries with nothing bound to it: the whole surface.
///
/// One rather than zero because a compound with nothing wired into its bias is
/// a compound the caller wants everywhere, and a default of zero would make a
/// standalone bake a picture of the substrate with nothing done to it.
pub(crate) const UNBIASED: f32 = 1.0;

/// UV units of relief per unit of height, assumed of a caller's height field.
///
/// A centimetre over a repeat of a metre and a half, which is between the
/// brick's nine millimetres and the plaster's four in the showcase. It is the
/// default of every compound's `relief` parameter and the
/// [`normal_strength`](crate::PbrOutput::normal_strength) each one bakes alone
/// with, so the picture a compound makes by itself is lit the way it says it
/// is.
pub(crate) const RELIEF: f32 = 0.01;

/// The gain that turns the curvature of a relief-scaled height into a mask.
///
/// [`Curvature`](crate::nodes::Curvature) answers a *difference of heights*
/// rather than a second derivative, so its magnitude is in the units of its
/// input. A height read as a length is a length: an arris standing a
/// centimetre proud of a metre-and-a-half repeat answers about a thousandth,
/// which is why the gain that makes a mask of it is in the hundreds rather
/// than the single digits the node's own default is written for. Eight hundred
/// saturates on an edge about that sharp at [`RELIEF`], and a caller who
/// declares a shallower relief gets a fainter mask, which is the point.
pub(crate) const CURVATURE_GAIN: f32 = 800.0;

/// The substrate channels nearly every compound reads, as inputs with the
/// defaults that make it bake alone.
///
/// Three rather than a configurable set: these are the three a weathering pass
/// always has an opinion about, the node ids are the input names so that
/// everything downstream reads them by the name the caller binds, and a
/// compound that wants the metalness or a bias as well adds
/// [`metallic_input`] or [`bias_input`] beside them.
pub(crate) fn substrate(builder: MaterialGraphBuilder) -> MaterialGraphBuilder {
    builder
        .node("height", GraphInput::float("height", FLAT_HEIGHT))
        .node("base_color", GraphInput::color("base_color", MID_GREY))
        .node("roughness", GraphInput::float("roughness", MATT))
}

/// The substrate's metalness, for a compound that changes it.
pub(crate) fn metallic_input(builder: MaterialGraphBuilder) -> MaterialGraphBuilder {
    builder.node("metallic", GraphInput::float("metallic", DIELECTRIC))
}

/// Where the caller will let this compound act at all.
///
/// A plain float, wired by whatever instances the compound: a
/// [`WorldMask`](crate::nodes::WorldMask) for weather that only falls on
/// upward faces, a live parameter for a puddle that comes and goes, a constant
/// for a surface that is weathered all over. It is an input rather than
/// something the compound reads itself because a bake refuses the world-space
/// inputs by path, and a compound that read one could never be baked.
pub(crate) fn bias_input(builder: MaterialGraphBuilder) -> MaterialGraphBuilder {
    builder.node("bias", GraphInput::float("bias", UNBIASED))
}

/// A height field as a length in UV units, which is what a neighbourhood
/// filter reads.
///
/// The study's `relief` helper, with the multiplier wired to a parameter
/// rather than fixed: a compound is instanced onto somebody else's height and
/// only they know how deep it is.
pub(crate) fn relief_of(height: &str, relief: &str) -> Math {
    Math::new(MathOp::Mul, height, Input::param(relief))
}

/// The mask an `amount` decides, as the four nodes that say it.
///
/// `clamp((field - (1 - amount)) / softness, 0, 1)`: at an amount of one
/// everything the field found is weathered, at zero only what the field
/// reported as a full one is, and `softness` is the width of the band between
/// the two — a hard scar at a hundredth, a rubbed patch at a half.
///
/// Four nodes rather than a [`Levels`](crate::nodes::Levels), which says the
/// same arithmetic in one, because `Levels` takes its bounds as fields of the
/// node: nothing a caller binds could reach them, and a threshold no caller
/// can turn is not a parameter. The helper is shared so that every compound's
/// `amount` means the same thing, which is what lets one be tuned by feel
/// after another.
///
/// The division is the crate's, so a `softness` of exactly zero answers zero
/// everywhere rather than a hard edge — which is why the parameter's range
/// starts above it and its default is a real width.
pub(crate) fn thresholded(
    builder: MaterialGraphBuilder,
    id: &str,
    field: impl Into<Input>,
    amount: impl Into<Input>,
    softness: impl Into<Input>,
) -> MaterialGraphBuilder {
    builder
        .node(format!("{id}_floor"), Math::new(MathOp::Sub, 1.0, amount))
        .node(
            format!("{id}_over"),
            Math::new(MathOp::Sub, field, format!("{id}_floor")),
        )
        .node(
            format!("{id}_ramp"),
            Math::new(MathOp::Div, format!("{id}_over"), softness),
        )
        .node(id.to_owned(), Clamp::new(format!("{id}_ramp")))
}
