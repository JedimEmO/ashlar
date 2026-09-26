# Delivering a material

A graph describes a surface; a `MaterialDefinition` in a `MaterialLibrary`
(`crates/ashlar-surface/src/material.rs`, re-exported by `ashlar`) says how a renderer gets it. The constants
(`base_color`, `roughness`, `metallic`, `emissive`, `tile_metres`, `uv_offset`)
live on the definition in every case and **multiply** into what the surface
provides; only the `surface` field changes between the three procedural
deliveries. A building recipe binds material keys and never knows which variant
is behind one.

## `Surface` variants

```ron
// Constants only. Glass, lights, trim.
"game:signal": (
    base_color: (0.55, 0.24, 0.045),
    roughness: 0.55,
    emissive: (0.8, 0.3, 0.05),
    surface: Plain,
),

// Baked into images when the material is registered; no file anywhere. This
// is every definition `ashlar_material::stdlib::materials()` hands out.
// Doubled parentheses because it wraps the same `Bake` struct.
"library:stone-cladding": (
    tile_metres: (2.0, 2.0),
    surface: Graph((
        graph: "library:stone-cladding",
        params: { "variation": Float(0.0) },
        resolution: 512,
    )),
),

// Files on disk that the content step (`ashlar_material::export`) wrote from
// that same definition. `baked_from` is provenance so the step can be re-run
// and the preview panel can find the graph behind a wall; it is not lowered at
// load and cannot be overridden per instance.
"library:formed-concrete": (
    tile_metres: (2.0, 2.0),
    surface: Files(
        base_color: Some("materials/library/formed-concrete/base.ktx2"),
        normal: Some("materials/library/formed-concrete/normal.ktx2"),
        orm: Some("materials/library/formed-concrete/orm.ktx2"),
        height: Some("materials/library/formed-concrete/height.ktx2"),
        emissive: None,
        baked_from: Some((graph: "library:formed-concrete", params: {}, resolution: 512)),
    ),
),

// Compiled into a fragment shader when the material is registered.
"showcase:concrete-wet": (
    tile_metres: (2.0, 2.0),
    surface: Shader(
        graph: "showcase:concrete-wet",
        params: { "wetness": Float(0.85) },
    ),
),
```

Every map of `Files` is optional. `base_color` is sRGB and multiplies the
constant; `normal` is a linear OpenGL tangent-space map; `orm` packs occlusion,
roughness and metallic into R, G, B; `height` is preflighted but not uploaded
(Bevy's `depth_map` reads the opposite convention, so parallax waits); an
`emissive` map multiplies the `emissive` constant, which defaults to zero, so a
map alone glows not at all. Prefer KTX2 for baked maps: a PNG carries level 0
only, Bevy builds no chain for it, and PNG cannot hold the 16-bit height plane.

`Bake` fields (`ashlar::Bake`):

| Field | Meaning | Check |
| --- | --- | --- |
| `graph` | key in the `MaterialGraphLibrary` | non-blank; must exist and lower at load |
| `params` | values written over the graph's declared defaults, by name | every value finite; unknown names or wrong types refused at load by `ashlar_bevy::read_library` |
| `resolution` | texels per repeat | power of two in `256..=4096`, and at least the graph's finest lattice |

### Strands are delivered separately, and the same way

A definition that grows strands names a **baked strand set** beside whatever
its surface is, and the surface may be anything at all — including `Plain`:

```ron
"lawn:field": (
    tile_metres: (2.0, 2.0),
    surface: Plain,
    strands: Some((
        layers: ["blades", "fibres", "stragglers"],
        lod_metres: [2.5, 7.0],
        card_metres: Some(14.0),
        density: 0.25,
        cast_shadows: false,
        baked_set: Some("materials/library/grass/set.strands"),
    )),
),
```

One file per definition, holding every layer it names; the file's own directory
is what finds a layer inside it. `ashlar_material::export` writes one for every
definition that grows strands and names no set, through `ashlar_strands::file::write`, and `ashlar_bevy::strands::StrandSets::read` is what a game calls.
The file is lossless, so the lawn a game draws is the lawn the content step
drew, triangle for triangle.

Leave `baked_set` out only where the caller holds the graph library and means
to scatter — which is the preview and the content steps, through
`strand-scatter`. A definition with neither a graph-naming surface nor a
`baked_set` is refused by `check_library`.

What one costs: `library:grass` is 24.09 MiB over three layers and 274 604
strands, and `library:moss-carpet` is 10.98 MiB over one and 125 166. A
strand is twenty-three `f32`. Reading the grass is 35 ms against 528 ms to
scatter it, and the dependency is the larger half of why the file exists. Thin
a set with the definition's own `density` before reaching for anything else: it
keeps a rank prefix, so it writes a shorter file of the same lawn.

The card impostors need **no second file**: the atlas is a pure function of the
set, so a game that holds one draws its own in about half a millisecond.

`Shader` has `graph` and `params` with the same rules and no resolution: the
static half is rasterised by `ashlar-bevy` at `shader::DEFAULT_RESOLUTION`
(1024) unless the `ShaderContext` says otherwise. The sheet's test checks that
the compiled half of a pair binds the bytes a bake of the same graph writes, to
within one code on a handful of texels.

## Preflight at load

`ashlar_bevy::read_library_with_graphs` takes the material library and a
`MaterialGraphLibrary` (`ashlar_bevy::read_graphs` reads one from RON), and
`check_library_with_graphs` does the same for a library already in memory, such as
`stdlib::materials()` merged with a game's own. Both validate every `Graph` and
`Shader` surface by lowering it: unknown graph key, undeclared parameter, cycle,
unlowerable node (Simplex, rounded brick corners), a world-space node in a
`Graph` graph, or a `Shader` graph binding more than eight images after packing
scalars four to an image, are all startup errors naming the material and the
graph path. A `Files` surface has its files inspected as KTX2 headers, not
baked, and a `baked_set` is opened and held to the layers the definition
names — under `strands` that is the file's own directory, and featureless it is
the identifier and the version, which is the same bargain the KTX2 preflight
makes.

## Per-instance overrides

A slot binding is either a bare key or a key with parameter overrides, and
both are the same field (`ashlar::Binding`):

```ron
instances: [
    (id: "tower/bay-0", part: "metro:bay", pose: (...), materials: {}),
    (id: "annex/bay-0", part: "metro:bay", pose: (...), materials: {
        "stone": (material: "showcase:corporate-stone", params: {"variation": Float(0.37)}),
    }),
],
```

```rust
instance.binding(
    "stone",
    Binding::new("showcase:corporate-stone").param("variation", ParamValue::Float(0.37)),
)
```

Rules, and where each is enforced:

- Only `Graph` and `Shader` surfaces take overrides. `MaterialLibrary::check_for`
  refuses an override on a `Plain` or `Files` surface at
  `instances[id].materials[slot]`.
- The names must be parameters the graph declares, at the declared type;
  `ashlar_bevy::read_library` checks by lowering the overridden surface.
- An override replaces values only. The graph, the resolution and the
  definition's constants stay the library's, so a per-instance *tint* is not
  this: two tints are still two keys.
- Overriding a slot replaces the palette's binding for it.

## Choosing a delivery

| Situation | Delivery | Why |
| --- | --- | --- |
| The default: a surface authored once | `Graph` | No files; one texture set per distinct bake at registration. What `stdlib::materials()` hands out. |
| The same, in a game that should not bake at startup | `Files` with `baked_from`, from `ashlar_material::export` | The same definition run through the content step: KTX2 with its chain, no bake at runtime. Nothing baked is committed in this repository; `just materials` regenerates it. |
| Variety per building: a seed, a cracking amount, a tint the game picked | `Graph` plus a `Binding` override | One key, one texture set in memory per distinct value, no files. |
| A value that moves per frame, or one a game changes at runtime | `Shader` with the parameter marked `.live()` | A uniform write; nothing re-rasterised. |
| Time, world position, world normal, cut flag, triplanar | `Shader` | A bake folds time and the cut flag to zero and refuses the rest. |
| A wall that is one material worn differently on cut faces | `Shader` reading `CutFlag`, not a second material key | One mesh, one draw; `showcase:concrete-cut-aware` is the example, and a `cut` slot stays for faces that really are another material. |

## What each costs

- **Files.** Exporting the whole library at its 512 defaults takes about forty
  seconds on eight threads; one material at 2048 is one to fifteen seconds,
  the cobblestone and its moss the slowest. A 1024 set is about 19 MiB
  uncompressed and 4 to 5 MiB with zstd; the 16-bit height is 40% of the
  compressed bytes and barely compresses. The GPU bakes the large graphs
  faster, and a graph it cannot split (`ashlar_bevy::gpu::dispatchable`, e.g.
  the grass) goes to the CPU.
- **Runtime bake.** Every distinct parameter set is its own texture set in
  memory; identical `Bake`s share one. A handful of distinct sets at 512
  bakes in a few seconds at startup (the corporate block's five took about
  three, measured 2026-09-15). Keep the bake
  off the main thread in a game. Bound images and planes are cached by the
  plane's closure (`planes::PlaneKey`), so two compiled graphs that instance
  one subgraph share its planes: the second of two compiled concrete
  variants over one wall costs 194 ms after the first instead of 1391
  (measured on the retired study concrete).
- **Shader.** Read the `CostReport` printed when the material compiles:
  `ops` per fragment, `samples`, `textures` (at most 8 after packing),
  `planes` the static half rasterises, `taps` for a live height's derived
  normal, `frozen` parameters, and the `verdict`. A live parameter at the last
  blend costs a blend per fragment; a live parameter that warps the outputs
  (a live `variation`) costs every coordinate-dependent instruction, roughly
  half the graph (`references/recipes.md`). A graph with no live inputs
  prints `no live inputs: bake this` and should be delivered as files or a
  `Graph`. Packed scalars go to half floats, trading the bake's own
  quantisation for fitting. A `Shader` override of a *folded* parameter is a
  second compiled graph and a second set of bound textures; an override of a
  *live* one is a different uniform block on the same pipeline. Each compiled
  material asset is its own bind group and its own draw: `GraphExtension` does
  not opt into Bevy's bindless path (2026-09-20), so a live value that differs
  per building is a draw call per building.
- **A live height** costs four neighbourhood taps per fragment for its normal
  and its roughness does not widen with distance (`widens: false`); no shipped
  material does this.
- The GPU bake is bounded by the device's storage buffer binding limit
  (`gpu.rs` reads `max_storage_buffer_binding_size` and `max_buffer_size`).
  With wgpu's default 128 MiB binding size, one output at 4096 is 256 MiB
  (sixteen bytes a texel) and does not fit, so a default device refuses above
  2048 and the error names the limit; outputs that fit are batched per
  dispatch. Lifting it means tiling a dispatch into row bands, which the kernel
  cannot do yet: it derives its UV from `global_invocation_id` with no offset. The CPU bake goes to 4096
  and is the reference.
