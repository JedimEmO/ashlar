---
name: ashlar-materials
description: Author, tune, compose, bake and visually review procedural PBR materials in this Ashlar repository. Use for new material graphs, reference reproductions, weathering variants, tiling fixes, material parameters and baked versus shader delivery.
---

# Authoring a material with ashlar-material

## The mental model

Treat a material as a graph of tiled fields. Bakeable graphs describe fields over
the UV coordinate; runtime graphs can additionally read world inputs and live
parameters. Generators lay a lattice that wraps at an integer period,
pointwise filters combine fields, resamplers read a field somewhere else, and
buffered filters read a neighbourhood. Wire them into one `PbrOutput`:
`base_color`, `roughness`, `metallic`, `occlusion`, an optional `height` and an
optional `emissive`. Build the graph to get a `Material`, which is the only thing
a backend consumes.

Hold four facts in mind while wiring:

- **Every port carries a period.** `Tiled { u, v }` says the field is laid on
  that many cells across UV `[0, 1)` and meets itself at the seam. Generators
  tile at their own period, pointwise nodes at the least common multiple of
  their inputs, transforms only through integer changes. The output must tile,
  and the material's period is the lcm across its outputs. A field that does
  not tile is fine inside the graph and an error at the output, named by node
  path.
- **Height drives the normal.** Nothing authors a normal map directly. Bind a
  `height` in `0..=1` and set `normal_strength`; the bake derives the normal by
  a wrapped central difference over the height plane, so relief, parallax and
  occlusion all read one surface.
- **Colour inside the graph is linear.** Every `Color` literal, `Colorize` stop
  and `Param::color` is linear RGB. The encoder applies the sRGB transfer to
  the base colour only. A float reaching a colour port broadcasts; a colour
  reaching a float port becomes luminance; a `Vec2` converts to nothing.
- **Three deliveries, one graph.** The same graph ships as KTX2 files a content
  step wrote (`Surface::Files` with `baked_from`), as images baked at
  registration (`Surface::Graph`), or as a compiled fragment shader
  (`Surface::Shader`). A bake and its compiled twin are the same lowering, so
  they share expression semantics. Still compare the rendered results: texture
  sampling, mipmaps, normal derivation and renderer setup affect appearance.

## Choose the existing workflow first

Paths below are relative to the repository root unless they start with
`references/`, which is relative to this skill.

- **The default library:** every finished material is a `library:*` graph in
  `crates/ashlar-material/src/stdlib/`, built from `layouts:*`, `substances:*`
  and `weathering:*` parts; `stdlib::graphs()` in `stdlib/mod.rs` registers them and
  `stdlib::materials()` gives each a `Surface::Graph` definition. The lineup and
  the reference per surface are `docs/material-references/lineup.md`.
- **Reference reproductions and their variants:** `library:brick`,
  `library:soi-cobblestone`, `library:grass`, `library:moss-carpet` and the moss
  variants, in `stdlib/reference_*.rs`. Read
  [references/fidelity.md](references/fidelity.md) for the visual loop and the
  commands. They were reviewed at 2048² over 2 m repeats; that is a review
  density, not a universal material budget.
- **Showcase-only materials:** `examples/showcase/src/materials.rs` holds the
  four compiled `showcase:*` graphs that read the world, and
  `examples/showcase/src/library.rs` the scene-specific definitions over the
  library. Nothing else belongs in the showcase.
- **Weathering or a variant of an existing material:** read
  [references/weathering.md](references/weathering.md). Instance the substrate
  and its exported masks; do not copy its recipe. The working brick and cobble
  moss wrappers are in `crates/ashlar-material/src/stdlib/reference_moss.rs`.

For visual work, first inspect the supplied reference and current render. Name
what needs changing: layout, relief, colour, roughness, coverage or fine texture.
Tests establish mechanics; they do not establish resemblance. Preserve aspects
of the composition the user has already accepted.

## The library workflow

1. **Write the builder.** Add a `pub fn name() -> MaterialGraph` in the
   `stdlib/*.rs` file of its family (or the game's equivalent), keyed
   `library:<name>`. Compose it from existing parts through `Subgraph` and
   `layer` before writing new nodes: a slab or board layout, a substance, a
   weathering compound. Declare `Param`s, add nodes under short snake_case ids,
   and finish through the family's `finish` helper or `.output(PbrOutput::new()
   ...)`. `references/recipes.md` points at the graphs worth copying. Write
   generator periods as powers of two that divide one another, so the
   material's period equals its finest lattice and no lcm warning fires.

2. **Register it.** Add it to the list in `stdlib::graphs()` in
   `crates/ashlar-material/src/stdlib/mod.rs`. `stdlib::materials()` derives a
   definition for every `library:*` key from the graph's own `tile_metres` and
   parameter defaults, so there is no second table. Add behaviour tests beside
   the others in `crates/ashlar-material/tests/surfaces.rs`.

3. **Pick a delivery.** Read `references/delivery.md`. The default is
   `Surface::Graph`, which bakes at registration: right for the preview and
   tests, and refused by a game with `ashlar-bevy`'s default features. A game
   ships files, which its content step writes (`ashlar-content`, or
   `ashlar_material::export` directly; `just materials` in this workspace), as
   `docs/guide/integration.md` describes. `Graph` plus a per-instance `Binding` override is a
   per-building seed or tint; `Shader` is for anything that moves per frame or
   reads time, world position, world normal or the cut flag. Mark a parameter
   `.live()` only when a game will move it at runtime; everything else folds.

4. **Bake it and read the report.** Export it with PNGs and read the line the
   export prints: `cargo run --release -j 8 -p ashlar-showcase --example
   export-materials -- target/pass 2048 <name> png`. For a shader, read the
   `CostReport` printed when the material compiles. Check three things:
   `height_range` sits inside `0..=1` without unintended clipping; its spread
   and physical scale suit the surface (a paint film need not span 0.1); the
   op count and plane count are what the graph meant to spend; the verdict is
   not `no live inputs: bake this` on a graph delivered as a shader.

5. **Iterate with the panel.** `just preview --scene sheet-<name> --graphs-out
   /tmp/tuned.ron`, press `M`, click a specimen to select the graph behind it,
   and drag. A live parameter of a compiled surface moves under the pointer;
   anything else re-bakes on release. `S` writes the library to the
   `--graphs-out` file; start the preview again with `--graphs /tmp/tuned.ron`
   and it reloads that file within half a second of an edit. A file that fails
   to parse or validate is reported and the last good library stays.

6. **Freeze the numbers.** Copy the tuned values back into the Rust builder,
   because the builder is the only source: nothing baked is committed, so there
   is nothing else to regenerate.

7. **Look at the sheet.** `sheet-<name>` shows the material at all three
   distances, baked beside compiled; a pair should match, and the far column
   shows what the mip chain keeps. `detail-<name>` is a two-metre patch up
   close. Then look at level 0 of the maps and at the studio sphere
   (`references/fidelity.md`), because the sheet is lit at 14 000 lux over
   twelve specimens and hides most of what a chip or a joint does.

## The rules that bite

Each of these is stated in full, with the check that enforces it, in
`references/nodes.md`; the line here is the reminder.

- **Periods are integers in `1..=4096`.** Transform keeps one only through an
  integer scale, a quarter turn or a translation; Mirror keeps the input's
  period unchanged; only a four-sector Kaleidoscope tiles; Tile tiles once
  unless its mask is free. See the transforms table.
- **An lcm warning is a mistake to fix.** A blend of period 8 with period 12
  builds at 24 and `Material::warnings` names it; the showcase test asserts
  every shipped graph has none. Keep periods as powers of two.
- **A bare `Uv` used as a value is seamed.** Feed it to generators or through
  `Math` `Sin`, never straight into a `Warp` offset.
- **Height must stay in `0..=1`.** The 16-bit map saturates while the normal,
  derived from the unclamped plane, keeps its slope. Read `height_range` in
  the report; bed the range with a `Levels` and divide the same factor back out
  of `normal_strength`, as the concrete does.
- **Convert physical relief to UV scale.** For a square repeat, use
  `normal_strength = height_scale_metres / tile_metres`. The library brick
  uses `0.04 / 2.0 = 0.02`; cobble uses `0.025 / 2.0 = 0.0125`.
  `normal_strength` defaults to zero, which silently produces a flat normal.
  Feed `OcclusionFromHeight` the height multiplied by the same UV relief.
  A coating's normalized `depth = thickness_metres / height_scale_metres`;
  copying the same depth between substrates with different scales changes its
  physical thickness. See `finish` in `stdlib/surfaces.rs` and `bake::derive_normal` for the
  implementation; do not interpret the API's “metres per repeat” as raw metres.
- **Declare the repeat you authored against**: `.tile_metres([1.72, 1.72])` on
  the builder. It is the `tile_metres` in the rule above and the scale a strand
  layer's absolute metres assume. It is advisory — nothing in a bake, a key or a
  shader reads it — and it is what `ashlar-bevy` compares a definition's own
  `tile_metres` with at `read_library`. A definition that lays the graph at another size
  on purpose, as the sheet does to fake distance, declares `tile_scale`; an
  undeclared scale and any change of the repeat's *shape* are warned about.
  Write the number once and read it from both sides: `stdlib::materials()`
  takes each definition's `tile_metres` from the graph's own declaration.
- **`Blend` opacity is a mix weight; `Mix` at zero is `a` bit for bit.**
  Dissolve is the exception. See the pointwise table.
- **`Levels` clamps to its input range before the gamma.** Use `Math` where
  the arithmetic must run free.
- **Angles are in turns** for `Math` trig and `DirectionalWarp`, in degrees
  everywhere else.
- **Buffered nodes are the bake boundary and cost a plane each**, shared where
  two nodes ask the same question. In a shader, a live parameter that reaches
  one is frozen and named; a runtime input above one is refused. See the
  buffered table.
- **Live parameters pull everything downstream into the fragment.** A live
  value at the last blend costs one blend; a live `variation` warping the
  outputs re-emits every coordinate-dependent instruction. Read the cost report
  before shipping; the measured numbers are in `references/recipes.md`.
- **Subgraph parameters fold, never live; subgraph *inputs* are whole fields.**
  Build with `build_in` or through the library, or the key is unknown. Use
  `layer` to give every requested output the same binding signature and allow
  sharing; a stray extra binding makes a distinct instance. Read the actual
  lowered op/plane report rather than assuming composition has zero cost. A bound
  field
  carries its type, period and lattice across the boundary, so a period-8 field
  through an inner `Transform` of scale 2 really does lay 16 cells.
- **A shipped compound never reads the world; the caller wires the bias.** Every
  `weathering:*` graph takes a plain float — `bias`, `wet`, `seed_mask`,
  `peel_mask` — and the graph that instances it puts a `WorldMask`, a live
  parameter or a constant in. See `references/weathering.md`.
- **Simplex is refused at lowering.** Use Perlin.
- **World-space inputs cannot bake**, even when nothing reads the node; `Time`
  and `CutFlag` bake as zero. Deliver such a graph as `Shader`, instancing the
  bakeable wall through `Subgraph` as `showcase:concrete-wet` does.
- **A bake needs texels for the finest lattice**, octaves, scales and scatter
  counts included. Keep shipped graphs at or under 512, because every library
  definition bakes at 512 when it is registered.
- **Bricks and tiles check their own arithmetic**: bond divides rows (write
  `1.0 / 3.0`), mortar under 0.5, hex rows even, herringbone counts divisible
  by four, rounded corners refused.
- **A `Shader` binds at most eight images**; scalars pack four to a half-float
  image first.
- **Bound bake concurrency.** Use four to eight threads per bake, as the
  existing writers/tests do. Run large bakes serially, and finish file exports
  before tests or renders read those files. One worker per core across
  concurrent bakes has exhausted memory.

## Growing strands from the same graph

A graph may carry named *strand layers* beside its `PbrOutput`, in
`MaterialGraph::strands`. A strand layer reads the graph's ordinary nodes once
per strand at its root rather than once per texel, and what comes out is
geometry: grass, fur, moss, carpet pile, thatch. `library:grass` is the worked
example and `library:moss-carpet` is the second one. See
[ADR 0004](../../../docs/adr/0004-strand-layers.md).

### Authoring a layer

Build the surface's own fields first, then read them from the layer. Nothing is
duplicated: `patches` decides where the pile is thick in the texture and in the
geometry, because both ask the same node.

```rust
.node("blade_flow", Direction::from_slope("clumps").rotate_quarter())
.node("blade_cover", remap(m(Add, m(Mul, "patches", 0.45), m(Mul, "clumps", 0.55)), 0.90, 1.0))
.strands(
    "blades",
    StrandLayer::new()
        .count(256)                       // the lattice, and part of the period
        .per_cell(1)                      // density without touching the period
        .seed(seed.wrapping_add(97))
        .density("blade_cover")           // a keep threshold, never a fade
        .length("blade_length")
        .lean("blade_lean")
        .bend("blade_bend")
        .direction("blade_flow")
        .colors("blade_root_color", "blade_tip_color")
        .metres(0.018, 0.0013)            // length, root width
        .clumps(64, 0.65, 0.45)           // lattice, share, tip pull
        .clump_tint(0.30)
        .midpoint(0.28)                   // where the blade is widest
        .facing_variation(0.85)
        .height_offset(0.018 * 0.16)
        .length_variation(0.40)
        .direction_variation(0.80)
        .segments(2)
        .taper(0.94)
        .root_occlusion(0.22),
)
```

- **The fields are `Input`s and obey the tiling rule.** `count` joins the
  material's least common multiple, so keep it a power of two beside the rest.
- **`density` is a threshold against the strand's own hash.** A value of 0.9
  keeps nine cells in ten; it does not make every strand nine tenths of a
  strand.
- **Raise `per_cell`, not `count`,** when you want more strands. `count` is a
  lattice the repeat has to carry; `per_cell` is not, because a cell's strands
  share its coordinate and differ only in the salt their hashes are taken under.
- **The `_variation` amounts are per strand; a wired noise is per neighbourhood.**
  A lawn wants both. `clump_share` moves any fraction of every variation onto
  the clump's hash, which is what makes a tuft agree with itself.
- **`StrandProfile::Blade` is a ribbon and two triangles a segment;
  `Fibre` is a three-sided tube and six.** Use `Fibre` for moss, bristles and
  anything seen from every side, and pay for it by halving the count.
- **Fields may not reach the frame or the mesh.** `Time`, `WorldPos`,
  `WorldNormal`, `CutFlag`, `Triplanar` and `WorldMask` are refused at the node
  that asked, and so is a `StrandRelief` of any layer. A `Live` parameter is
  folded at its bound value and named in `Partition::frozen`.

### The relief: the same strands, seen from above

`StrandRelief` names a layer of the *same* graph and a repeat size in metres,
and splats the scatter into a plane the PBR half reads. It is what keeps the
texture and the geometry one surface, so build the surface out of it rather
than out of a second drawing of blades.

| Output | What it is | What to build with it |
| --- | --- | --- |
| `Coverage` | The union, saturating at one | Mix the strand colour up over the bed |
| `Mass` | Every contribution summed | The pile the strands stand in |
| `Height` | The topmost contribution, in units of `length_metres` | How high the canopy stands, which is the light |
| `Along` | Coverage-weighted position along the strand | A tip band |
| `Color` | Coverage-weighted strand colour | The base colour of the strands |
| `Id` | The topmost strand's free hash | Per-strand tint |
| `Occlusion` | `Mass` weighted by the height each contribution stood at | How deep the canopy is |

Two rules that cost two passes to learn:

- **A canopy is dark underneath and bright on top.** Mix *up* from a dark bed
  towards the strand colours by coverage, then shade by `Height`. A graph that
  starts from the lit bed and darkens where a strand stands puts the shadow on
  the one thing that is in the light, and the close-up is a pale floor with
  black scratches on it.
- **Do not put a raw `Height` into the height map.** It is the topmost
  contribution and nothing else, so it steps from a full strand to bare surface
  across one texel, and a step like that at a millimetre a texel is a vertical
  wall that saturates the normal map. Multiply it by the coverage that drew it,
  and let the smooth `Mass` carry most of the range.
- **A strand narrower than one texel of the bake cannot be splatted.** The bake
  warns by path. At 2048 over a two-metre repeat that floor is about 1 mm.

### Growing it on a surface

`MaterialDefinition::strands` is an optional `StrandSettings`:

```ron
strands: Some((
    layers: ["blades", "fibres"],
    lod_metres: [2.5, 7.0],
    density: 1.0,
    cast_shadows: false,
    card_metres: Some(14.0),
    // Where the baked set lives. A game names this and reads the layers off
    // disk; a tool that holds the graph library may leave it out and scatter.
    baked_set: Some("materials/library/grass/set.strands"),
)),
```

- **`baked_set` is how a game gets the layers at all.** A strand set is
  scattered from the graph, which is the tool half; the file is what a game
  reads instead, and `ashlar-bevy`'s `strands` is a *game* feature that pulls
  no graph engine. The content step writes one per definition holding every
  layer it grows (`ashlar_material::export`), `read_library` opens and
  validates it before a window exists, and `examples/lawn.rs` in `ashlar-bevy`
  is thirty lines between a file and a lawn. Leave it out only where the
  caller holds the graph library — the preview does, and scatters. See the
  2026-09-20 amendment to ADR 0004.
- **A level of detail is a rank prefix**, widened by `1/sqrt(keep)` and built
  from one segment fewer. Past the last distance there is either a card level or
  nothing, and past *that* what the camera sees is the relief, which is why the
  relief has to be the same strands.
- **`card_metres` is where the blades become pictures of themselves.** Every
  clump becomes two crossed quads wearing an atlas of the same set drawn side
  on, as `AlphaMode::Mask` and never blend, with a mip chain rescaled to keep
  its alpha coverage. It has to be past the last `lod_metres` entry and there
  has to be one; validation says both by path. Set it where a tuft is still
  worth a couple of pixels — 14 m for an 18 mm pile, further for anything
  taller — and leave it out where the relief alone is enough. Cards are a plain
  `StandardMaterial`, so a scene that grows them carries two material handles
  per layer and the cards do not sway.
- **`density` is the same cut applied up front.** Lower it on a scene that
  cannot afford the authored layer; it removes strands and never moves the ones
  that stay.
- **Turn `cast_shadows` off on anything dense.** A strand is a small fraction of
  a shadow texel, so what a lawn contributes to a shadow map is blocky black
  holes rather than dappling.
- **Add `ashlar_bevy::wind::StrandPlugin`** and spawn through
  `strands::strand_wind_material`. `StrandWind::still()` is zero strength and
  renders byte-identically to a plain `StandardMaterial`, which is what a
  reproducible capture wants.

### What it costs

Count it before building it: `set.len() * set.shape().triangles_per_strand()`.
`library:grass` is 688 818 triangles over one full-detail two-metre repeat
across three layers — a coarse `Blade` layer at one per cell and two segments,
a fine one at four per cell and one segment, and a sparse upright one on half
the lattice that exists to break the silhouette. Keep a repeat under about
800 000, say the number in the material's own doc comment, and assert it in a
test so the two cannot drift apart. The card
level adds about 16 000 on top of that, two quads for each of the repeat's
clumps, and one 256² atlas pair per scatter however many walls wear it.

## Commands

```sh
# A sheet of one library material, three distances, baked beside compiled,
# saving panel edits to a scratch file. M opens the panel, S saves.
just preview --scene sheet-brick --graphs-out /tmp/tuned.ron
# Reload the tuned file on every save, over the same scene.
just preview --scene sheet-brick --graphs /tmp/tuned.ron
# A two-metre patch up close.
just preview --scene detail-brick

# The content step: every material, or one, as KTX2 (and PNG) files under an
# asset root. Prints ops, planes, time and height_range per set. Nothing it
# writes is committed; `assets/materials/` is ignored.
just materials
cargo run --release -j 8 -p ashlar-showcase --example export-materials -- target/pass 2048 brick png

# The same maps lit on the studio sphere, from that export's PNGs.
cargo run --release -p ashlar-preview --example material-swatch -- \
  library:brick target/pass/brick-sphere.png sphere target/pass

# Point the preview at a library of your own.
just preview --scene corporate-block --graphs path/to/graphs.ron --materials path/to/library.materials.ron

# The whole content step, materials and buildings, as a game ships them.
just content 512

# The whole gallery, or one sheet, headless into target/references/.
just references
just references --reference-scenes sheet-plaster

# The library's graphs, their behaviour and the reference reproductions.
cargo test -p ashlar-material --test stdlib --test surfaces --test references -- --test-threads=1

# CPU against GPU, per op and per library graph. Needs an adapter.
just conformance

just ci
```

## References

- `references/nodes.md`: every node with its builder, fields, defaults,
  output type, period rule and backend support, one table per family.
- `references/recipes.md`: which library graphs to copy from for which kind
  of surface, and how a graph looks written out as RON.
- `references/delivery.md`: the `Surface` variants with RON, `Bake` fields,
  per-instance `Binding` overrides, and what each delivery costs.
- `references/fidelity.md`: reference analysis, material scale, visual diagnosis,
  export and verification commands.
- `references/weathering.md`: the standard library compounds and patterns,
  with each one's inputs, parameters
  and exported mask, a wiring example over the library brick using `layer`, and
  the world-bias idiom.

Read the doc comments in `crates/ashlar-material/src/nodes/*.rs` for any node
whose behaviour matters more than the table says; the code is the truth. The
long guide is `docs/guide/materials.md`, and what reads well on a lit surface
and what does not is `references/fidelity.md`. Two things the specimen sheets
taught (2026-09-15) that hold for any tiling map: a map cannot carry variation
coarser than its own repeat, so at a sheet's far column every material slides
toward its mean tone, and the fix is not in the map but a per-instance
parameter or a coarser field over it; and broad light-and-dark variation inside
one repeat is exactly what shows the repeat as a lattice at distance, so broad
interest and hidden tiling pull against each other. Treat palette values in the
example builders as examples, not universal albedo targets. Judge the material under the intended lighting and at its intended
viewing distances; inspect mipmaps as well as level zero.
