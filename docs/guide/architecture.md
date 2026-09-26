# Architecture

ashlar is seven crates, and the first thing to know about them is which half each belongs to.
The *content* half runs when content changes and writes files.
The *game* half loads those files and draws them, and never links the content half.

## The crates

| Crate | What it is | Half |
| --- | --- | --- |
| `ashlar` | The domain: parts, elements, instances, sockets, slots, rooms, terrain fitting, levels of detail, and the `.ashlar` baked building file | both |
| `ashlar-surface` | The material vocabulary both halves share: `MaterialDefinition`, `Surface`, `Binding` | both |
| `ashlar-strands` | Strand sets: placing, meshing and reading baked grass, fur and moss | game |
| `ashlar-bevy` | The Bevy adapter: `AshlarPlugin` for the game, tool features for authoring | game |
| `ashlar-manifold` | The mesher, over the native Manifold kernel | content |
| `ashlar-material` | Material graphs, the default library, the bake and the KTX2 export | content |
| `ashlar-content` | The content step as a library: bake a game's buildings and materials to files | content |

Each crate depends only on the ones listed before it here:

```text
ashlar-surface    nothing of ours
ashlar-strands    nothing of ours
ashlar            ashlar-surface
ashlar-manifold   ashlar
ashlar-material   ashlar-surface, ashlar-strands
ashlar-content    ashlar, ashlar-manifold, ashlar-material
ashlar-bevy       ashlar; ashlar-strands under `strands`; ashlar-material only under the tool features
```

`ashlar` knows nothing about Bevy, files or kernels, so a dedicated server can depend on it alone.
`ashlar-bevy` with default features depends on neither `ashlar-manifold` nor `ashlar-material`,
so a game has no C++ toolchain and no graph engine in its build.
A test in `ashlar-bevy` holds that, because a feature added in the wrong place would quietly undo it.

## How content flows

A building starts as a `Building`, built in Rust and validated by `build()`.
`ashlar_manifold::bake` meshes it at every level of a `LodPolicy` ladder,
simplifying the *recipe* for each coarser level rather than decimating triangles,
and writes a `BakedBuilding` into an `.ashlar` file.

A material starts as a `MaterialGraph`, a graph of tiled fields with a PBR output.
A `MaterialDefinition` names it through `Surface::Graph`,
and the content step bakes it to KTX2 maps with their mip chains
and rewrites the definition to name those files through `Surface::Files`.

`ashlar_content::Content::ship` does both in one call, and resolves per-instance parameter overrides into definitions of their own first,
because a file cannot take a parameter at runtime.
In the game, `AshlarPlugin` loads the `.ashlar` file and the `library.materials.ron` as assets
and spawns every level with its distance band, the collision proxies and the rooms.

The tool path is the same pipeline without the files.
`ashlar-bevy`'s `runtime-bake` feature bakes a `Surface::Graph` into images when the material is registered,
and `shader` compiles a graph into a fragment shader for parameters that change per frame.
That is what the preview and the web demo use, and what a game should not.

## Where things live

`crates/` holds the seven crates, each with its own README and its own examples.
`examples/showcase` is the kits and their scenes, `examples/integration-content` and `examples/integration-game` are the split a game copies,
and `examples/web` is the browser demo.
`tools/ashlar-preview` is the authoring tool.

`docs/guide` is this manual's chapters, `docs/adr` the decision records,
and `docs/material-references` the reference material the benchmark reproductions were matched against.
`.agents/skills` holds the two skills an AI agent working in this repository loads,
one for geometry and one for materials, which are also a compact reference for a person.

The `justfile` is the front door for commands: `just --list` shows them,
and `just ci` is what continuous integration runs.

## Words we use

**Part, element, instance.**
A *part* is a reusable solid made of named *elements*, each a `Geometry` expression with a material slot, a UV mode and a collision declaration.
A building places parts as *instances*, by pose or by *socket*.

**Slot, binding, key.**
An element names a *slot*, such as `wall`.
A building *binds* each slot to a material *key*, such as `library:brick`, and an instance may override a binding for itself, parameters included.
The part never learns what it wears.

**Graph, definition, library.**
A *graph* is a procedural material.
A *definition* says how a surface is drawn: its constants and its `Surface`.
A *library* is a map of keys to definitions, and a *graph library* a map of keys to graphs.

**Baked.**
Baked always means the content step's output, a file a game ships:
a *baked texture set* is a material's KTX2 maps, a *baked building* an `.ashlar` file,
and a *baked strand set* a `set.strands` file.

**Merged, standalone, far.**
A *merged* building unions its elements by *merge group*, usually a storey, into one solid.
A *standalone* element stays its own mesh anyway: glass, lights, doors.
A *far* element stays at every level of detail, however thin: a neon line, a crown of lights.

**Room, portal.**
A *room* is a declared box volume with a storey, and a *portal* an opening a door or window cutter publishes.
Neither is meshed; they are what AI navigation, audio and "which room is the player in" read.
