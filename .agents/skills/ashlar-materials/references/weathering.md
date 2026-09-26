# The standard library: weathering compounds and pattern recipes

`ashlar_material::stdlib::graphs()` answers a `MaterialGraphLibrary` of ten
graphs the crate ships: six weathering compounds keyed `weathering:<name>` and
four pattern recipes keyed `patterns:<name>`. They are built in Rust in
`crates/ashlar-material/src/stdlib/{weathering,patterns,moss}.rs`, and the doc
comment on each builder is the truth about why every number is what it is; this
page is the shape of the interface and the idioms for wiring one in.

Merge them into a game's own library and name the keys from `Subgraph` nodes:

```rust
let mut library = my_game::graphs();        // the game's own
library.extend(&ashlar_material::stdlib::graphs())?;   // refuses a key collision
```

## What a compound is, and what it is not

- **It takes the substrate's channels it intends to change**, as `GraphInput`
  nodes whose defaults make it bake to a plausible picture with nothing wired
  in. That is what lets you build, `check`, bake and preview one on its own —
  and it is why a standalone bake of `weathering:rust` is a plate rusted through
  rather than a wall with a rust patch on it: a constant input cannot draw a
  boundary, so the terminal state is what you see and the edge is what you wire.
- **It writes back the channels it names and no others.** `dirt_dust` binds no
  height, because a film of dust is microns on a wall whose relief is
  centimetres; reading `SurfaceOutput::Height` off it is a resolver error rather
  than a silent passthrough. The four channels `PbrOutput` binds unconditionally
  are the exception the type forces — read the channels the table below lists
  and nothing else.
- **It exports the mask it decided with** as an extra, read with
  `SurfaceOutput::Extra("name".to_owned())`. Layer on that rather than building
  a second mask that agrees with the first to within a texel.
- **It never reads a runtime input.** A bake refuses world-space inputs such as `WorldMask`, `WorldPos` and
  `WorldNormal`; `Time` and `CutFlag` bake as zero. Keep all of these in the
  caller so the compound can be tested as a reusable field. World-space bias is the caller's: see the
  idiom below.
- **A parameter cannot be a node field.** A filter radius, a noise seed, an
  octave count and a generator's period are fields, and nothing can wire a value
  into one, so those are documented constants in the builders rather than
  parameters. If a number you want to turn is not in the tables here, it is a
  constant on purpose; change the *input* you feed the compound instead.

## Two conventions that run through all of them

**`amount` is a contour, and `softness` is its ramp.** Every threshold in the
library is `clamp((field - (1 - amount)) / softness, 0, 1)` over whatever field
that compound decided with. So `amount = 0` is nothing at all, `amount = 1` is
everywhere the field is positive, and `softness` is how many units of field the
transition takes. `moisture`'s `level` is the same helper over the inverted
height, which is why a waterline reads like every other amount here.

**`relief` is UV units per unit of height.** A `Curvature` and an
`OcclusionFromHeight` read their input as a *length* in the same units as UV,
while a height input is a `0..=1` field. The compounds that read a neighbourhood
of the height therefore take a `relief` parameter and multiply by it first —
exactly the study's `relief` helper — and it is the same number you pass to
`PbrOutput::normal_strength`. Pass the caller's own figure (the brick's 0.009,
the plaster's 0.004) rather than leaving the 0.01 default, or the compound wears
a shallow surface as though it were a deep one.

## The weathering compounds

Defaults are in parentheses; a range follows where the parameter has one.

### `weathering:edge_wear`

Paint, patina and fired skin knocked off the edges that stick out. Nothing tells
it where an edge is: `Curvature::peaks` over the caller's height finds what
stands proud, a coarse Perlin breaks that line into knocks rather than a stripe
down every arris, and a slope `Blur` against the **inverted** height drags each
knock downhill onto the flank below it.

- Inputs: `height` (0.5), `base_color` (mid grey), `roughness` (0.8),
  `metallic` (0.0), `substrate_color` ([0.56, 0.57, 0.58], bare steel),
  `substrate_roughness` (0.35), `substrate_metallic` (1.0), `bias` (1.0).
- Parameters: `amount` (0.45, 0..=1), `softness` (0.12, 0.01..=1), `depth`
  (0.1, 0..=1, height taken off a chip), `relief` (0.01, 0..=0.5).
- Writes: base colour, roughness, metalness, height. Extra: **`mask`**.
- The substrate defaults are bare steel because that is the case where wear has
  to read. A masonry caller binds its own fresh clay or lime into the three
  `substrate_*` inputs and gets the brick's chip back.

### `weathering:dirt_dust`

Dust and dry dirt settled wherever the surface gave it somewhere to sit. Shelter
is asked twice, because one filter cannot answer it at both scales:
`OcclusionFromHeight` marches a horizon and finds the wide hollows, `Curvature::cavity`
finds the tight ones the march steps over, and the two are combined by a max —
dust sits wherever *either* kind of shelter exists.

- Inputs: `height` (0.5), `base_color` (mid grey), `roughness` (0.8),
  `bias` (1.0).
- Parameters: `amount` (0.75, 0..=1), `softness` (0.25, 0.01..=1), `color`
  ([0.20, 0.185, 0.16]), `dust_roughness` (0.96, 0..=1), `relief` (0.01,
  0..=0.5).
- Writes: base colour, roughness. **No height, no metalness.** Extra:
  **`cavity_mask`**.
- `relief` and `amount` are coupled. The compound reads the height as a
  length, so over a shallow surface (a plate 3.5 mm deep) the shelter field
  tops out near 0.29 where a raked brick wall saturates, and `amount` has to go
  far above its default. Tune the two together.

### `weathering:rust`

Iron oxide creeping out of wherever the finish has already failed. This one
takes a *place* rather than an amount: `seed_mask` is where the steel is already
open, and everything else is measured from it with a `Distance` plane.

- Inputs: `height` (0.5), `base_color` (mid grey), `roughness` (0.8),
  `metallic` (0.0), `seed_mask` (**1.0** — seeded everywhere, see above).
- Parameters: `spread` (0.08, 0..=0.25, how far out of the seed the front
  stands), `pitting` (0.15, 0..=1), `blister` (0.06, 0..=1, the lift along the
  front), `edge_color` ([0.175, 0.062, 0.020], fresh oxide),
  `core_color` ([0.045, 0.022, 0.014], old scale).
- Writes: base colour, roughness, metalness, height. Extra: **`rust_mask`**.
- Wire `seed_mask` from another compound's extra — `peeling_paint`'s `bare`, an
  `edge_wear` `mask` — or from a `Scratches` field or a band of `Noise` along a
  gutter line. `spread`'s ceiling is the `Distance` node's fixed range; past it
  the plane saturates and the front would stop moving.
- The mask does not reach one. `blotch` is a Perlin lifted to a floor of 0.35
  and multiplied into the creep, so the mask peaks near two thirds and never
  passed 0.9 over a 1024 tile (2026-09-16): about a third of the substrate's
  colour survives in every rusted texel. The oxide colour is mixed from
  `edge_color` toward `core_color` by that same mask, so `edge_color` shows
  where rust is thin rather than at a front. Over a light substrate, hand in
  oxide colours brighter and more saturated than oxide is.

### `weathering:peeling_paint`

Paint that has lost its key and is coming off in sheets. Also takes a place:
`peel_mask` says where the film has already gone, and the compound adds
everything that happens at the *edge* of that patch.

- Inputs: `paint_color` ([0.030, 0.072, 0.115], the study's panel blue),
  `substrate_color` ([0.56, 0.57, 0.58]), `paint_roughness` (0.5),
  `substrate_roughness` (0.35), `height` (0.5), `peel_mask` (**1.0**).
- Parameters: `lip` (0.08, 0..=1, height added across the curl),
  `curl_width` (0.012, 0..=0.08 UV).
- Writes: base colour, roughness, occlusion, height. Extra: **`bare`** (the hard
  mask of the exposed patch, which is what to feed `rust`'s `seed_mask`).
- It deliberately does **not** drop the height inside the patch by a film
  thickness; that is a number only the caller knows. The undercut under the curl
  is written into `occlusion`, because it is shadow rather than shape and a
  derived normal has no way to say it.

### `weathering:moisture`

Rain, standing where the surface let it and clinging where it did not. `level`
is a waterline in the caller's `0..=1` height; below it is submerged, above it
is damp. Colour is darkened by `exp2(-darkening * depth)` — an absorption, so a
deep corner goes darker than a shallow one for free.

- Inputs: `height` (0.5), `base_color` (mid grey), `roughness` (0.8),
  `metallic` (0.0), `wet` (**1.0**).
- Parameters: `level` (0.35, 0..=1), `softness` (0.05, 0.01..=1), `darkening`
  (2.5, 0..=8), `porosity` (0.5, 0..=1).
- Writes: base colour, roughness, metalness, height. Extra: **`wet_mask`** (the
  puddle only, not the damp term).
- Height is raised to the waterline where submerged, which flattens the derived
  normal across a puddle for free — a flat normal under a near-mirror roughness
  is what a still surface is.
- **It contains no buffered filter and takes no `relief`**: every filter in it is
  pointwise. That is why a live `wet` costs a handful of fragment instructions
  and freezes nothing, and it is the compound to reach for when the weather has
  to move at runtime.

### `weathering:moss`

A raised coating with a separate habitat mask and surface-detail fields.

- Inputs: substrate `height`, `base_color`, `roughness`, `metallic`, plus
  `bias` (1, exclusion gate), `shelter` (0.5, growth preference), `coverage`
  (1, multiplies the amount control).
- Parameters: `amount` (0.55), `softness` (0.10), `depth` (0.04, normalized
  height), `dark_color` and `light_color` (linear RGB).
- Writes: base colour, roughness, metalness, height. Extra: **`mask`**.
- Coarse colonies determine placement; warped cushions and finer shoots/tips
  determine texture. Changing surface fidelity need not change the habitat.
- Bind `coverage` to the wrapper's `moss_amount` input and set the inner
  `amount` to 1 when the wrapper owns the amount control. A subgraph parameter
  is a fixed value; use a graph input to pass a caller's field or live parameter.
- Use the substrate's actual masks: the library brick exports mortar,
  joint-depth and missing-brick masks; cobble exports stone ownership and dirt.
  `1 - own` prefers cobble grout. Shared UV coordinates alone do not create
  that relationship.
- Restore the substrate's `normal_strength` on the wrapper and derive AO from
  the combined height. For 2.6 mm maximum added relief, normalized depth is
  `0.0026 / 0.04 = 0.065` on brick and `0.0026 / 0.025 = 0.104` on cobble.
  The pile profile determines the actual local thickness; leave height headroom.
- Read `crates/ashlar-material/src/stdlib/reference_moss.rs` for complete
  working wiring. Both variants reference the original substrate graph. At zero
  amount `crates/ashlar-material/tests/references.rs` checks exact recovery,
  including AO and normals.

## The four patterns

A pattern draws a field rather than changing one, so it declares no channel
inputs and binds whatever makes its own bake a picture of itself. What you
usually want off one is its extra.

- **`patterns:radial_gradient`** — parameters `inner` (0.0, 0..=0.5) and
  `radius` (0.5, 0..=0.5), both radii in UV, read as a contour off a fixed
  `CircleMap` window. Writes base colour, roughness, height. Extra:
  **`gradient`**. `inner` above `radius` inverts it; the two equal is a divide
  by zero, which the crate answers with a flat zero.
- **`patterns:angular_gradient`** — parameters `offset` (0.0 turns, 0..=1) and
  `twist` (0.0, -4..=4 turns per unit radius). Writes base colour, roughness.
  Extra: **`gradient`**, deliberately the same name as the radial one so either
  can be swapped in without rewiring. **Binds no height**: a sweep wraps from one
  to zero along the ray at `offset`, and a step in a height is a wall in the
  derived normal.
- **`patterns:wood`** — parameters `warp` (0.06 UV), `fibre` (0.015 UV),
  `sharpness` (2.2, 0.2..=8), `early_color` ([0.235, 0.140, 0.072]),
  `late_color` ([0.085, 0.043, 0.022]). Writes base colour, roughness, height
  (bedded 0.35..0.65). Extra: **`rings`**, one in the late wood. Its fibre lays
  256 cells down the board, so **bake it at 256 texels per repeat or finer**;
  coarser is refused by the lattice rule.
- **`patterns:cracks`** — input `mask` (1.0) gating where cracks appear;
  parameters `width` (0.004 UV), `depth` (0.35), `chamfer` (0.012, 0..=0.05 UV),
  `warp` (0.03 UV), `face_color` ([0.42, 0.40, 0.37]), `crack_color` ([0.020,
  0.018, 0.016]). Writes base colour, roughness, height. Extra:
  **`crack_mask`**. An empty `mask` bakes the uncracked face, which is the right
  terminal behaviour.

## Wiring one in: `layer` over the library brick

A compound answers a *surface* rather than a field, so each output is a
`Subgraph` node of its own — and their bindings have to agree exactly, because an
instance is inferred per binding signature and a fifth input on one of four
near-identical nodes would quietly make it a second instance of the same graph.
`MaterialGraphBuilder::layer(id, subgraph, outputs)` writes them from one node,
adding `<id>.<port>` per listed output, the port being `base_color`, `height` or
an extra's own name.

```rust
use ashlar_material::{
    Input, MaterialGraph, Param, ParamValue, PbrOutput,
    nodes::{Subgraph, SurfaceOutput, WorldMask},
};

const BRICK_RELIEF: f32 = 0.02; // 40 mm over the brick's 2 m repeat

fn brick_weathered() -> MaterialGraph {
    MaterialGraph::builder("game:brick-weathered")
        // Live: the rain the game moves. Default zero, so an untouched
        // material is the dry wall exactly — everything `moisture` does is a
        // `Mix` by a mask, and at zero that is the identity.
        .param(Param::float("wet", 0.0).range(0.0, 1.0).live())
        // The wall itself: three channels off one instance of the library brick.
        .layer(
            "brick",
            Subgraph::new("library:brick"),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Height,
            ],
        )
        // Soot where the world lets it settle: the faces that point up. The
        // compound takes a plain float; the caller decides what it means.
        .node("upward", WorldMask::up().softness(0.35))
        .layer(
            "dirt",
            Subgraph::new("weathering:dirt_dust")
                .input("height", "brick.height")
                .input("base_color", "brick.base_color")
                .input("roughness", "brick.roughness")
                .input("bias", "upward")
                // The brick's own relief, so the cavity filter reads a length.
                .param("relief", ParamValue::Float(BRICK_RELIEF)),
            &[SurfaceOutput::BaseColor, SurfaceOutput::Roughness],
        )
        // Then the water, over the dirt: colour and roughness chain, and the
        // height is still the brick's, because dust did not move the surface.
        .layer(
            "wet",
            Subgraph::new("weathering:moisture")
                .input("height", "brick.height")
                .input("base_color", "dirt.base_color")
                .input("roughness", "dirt.roughness")
                .input("wet", Input::param("wet")),
            &[SurfaceOutput::BaseColor, SurfaceOutput::Roughness],
        )
        .output(
            PbrOutput::new()
                .base_color("wet.base_color")
                .roughness("wet.roughness")
                // The brick's own height, not `moisture`'s flooded one. See
                // below: on a wall it is the cheaper *and* the better picture.
                .height("brick.height")
                .normal_strength(BRICK_RELIEF),
        )
        .into_graph()
}
```

That wall reads a world normal, so it ships as `Surface::Shader` rather than as
files. Measured at 1024 texels with `wet` live over the study brick this page
was written against: **fifty-one instructions and one uniform per fragment**, nine texture reads over eight bound images built from
seventeen planes, no neighbourhood taps, nothing frozen. The brick and the dirt
are dry arithmetic over the coordinate and both went into the textures; only
`moisture`, downstream of the live uniform, is left in the fragment.

**Binding a compound's height costs more than it looks.** Take `moisture`'s
`height` output instead of the brick's — `.height("wet.height")`, with
`SurfaceOutput::Height` added to the `wet` layer — and the same graph is
**ninety** ops with **four neighbourhood taps**, because a height that depends on
a live uniform is a normal the fragment has to finite-difference for itself. Worse
than the ops: `CostReport::widens` goes false, so the roughness keeps its
level-zero value at every distance. On a wall whose wet joints sit at a
roughness of 0.02 that is a glitter which outstays its detail. The flooded height
is worth it for a floor, where a puddle really is flat; on something standing up,
where water clings and runs, take the substrate's own height and let the normal
stay a plane.

`examples/showcase/src/materials.rs::brick_weathered` is this graph as the
showcase actually ships it, as `showcase:brick-weathered`: the same wiring with the brick's occlusion bound as well — which
is what lets the packer put nine values in **five** images rather than eight in
eight — and two numbers overruled, a soot colour and a porosity, each with its
reason written where it is declared.

## The world-bias idiom

A compound cannot read the world, so the gate is a plain float input the caller
wires — `bias` on `edge_wear` and `dirt_dust`, `wet` on `moisture`, `seed_mask`
on `rust`, `peel_mask` on `peeling_paint`, `mask` on `patterns:cracks`. Three
things go into one:

- **A `WorldMask`**, for weather that only falls on some faces:
  `WorldMask::up()` for rain and dust, `WorldMask::below(0.6)` for a splash
  zone or rising damp, `WorldMask::facing(WorldAxis::NegZ)` for a sheltered
  elevation. This makes the graph a `Shader`; a bake refuses it by path.
- **A live `Param`**, for weather a game moves: `Input::param("rain")` with the
  parameter marked `.live()`. `moisture` is built for this — pointwise
  throughout, so the live value freezes nothing and costs tens of instructions.
- **A constant or another field**, for a surface weathered all over, or gated by
  something the graph already computed: a `Noise` along a gutter line, another
  compound's extra, a `Scratches` field.

Multiplying two of them is the usual composition: a `WorldMask::up()` times a
live `rain` is "rain falls on upward faces, as much as the game says". Keep the
product in the caller's graph and hand the compound one number.

## Testing a compound you wired in

The crate's own `tests/stdlib.rs` already holds every shipped graph to building
alone, tiling, warning about nothing, and baking at the test resolution with its height inside
the unit, so a caller's test is about *their* wall. Worth asserting:

- The material still tiles and `warnings()` is empty. A compound instanced under
  a `Transform` multiplies periods, and an lcm warning is a mistake to fix.
- The height stays in `0..=1`. `edge_wear` cuts `depth` out of it and `rust` and
  `peeling_paint` add to it; all three clamp, but a substrate already at the top
  of the unit has nowhere to go and the wear will read as nothing.
- Where the compound says it changed nothing, the plane is what it was handed
  **exactly**. Everything a compound does is reached through a `Mix` by its
  mask, so a mask of zero is the caller's own field bit for bit.
- The bake resolution covers the finest lattice: `weathering:rust`'s pits are 64
  cells and `patterns:wood`'s fibre is 256.
