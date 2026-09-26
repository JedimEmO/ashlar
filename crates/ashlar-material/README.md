# ashlar-material

Procedural PBR materials as ordinary Rust data.
We wire noises, patterns, transforms and blends into a PBR output,
and the crate checks that the result tiles,
bakes it into a texture set, or compiles it into a shader.
There is no Bevy in here, no file IO and no node editor.

It also ships a default library of around forty finished surfaces,
from formed concrete and brick to corrugated steel, curtain walls and grass,
so a game can start from real materials and replace them one at a time.

## Adding it

The crate is not on crates.io yet, so we depend on it through git:

```toml
[dependencies]
ashlar-material = { git = "https://github.com/JedimEmO/ashlar", features = ["zstd"] }
```

`zstd` is the only feature.
It turns on supercompression in the KTX2 writer,
which is what the content step uses; leave it off in anything that only bakes in memory.

A game at play does not need this crate at all.
The content step bakes materials to KTX2 files once,
and the game loads those through `ashlar-bevy` with no graph engine linked.
[The integration guide](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/integration.md) walks through that split.

## A graph

```rust
use ashlar_material::{
    MaterialGraph, PbrOutput,
    nodes::{Colorize, Levels, Noise},
};

let concrete = MaterialGraph::builder("game:concrete")
    .node("coarse", Noise::value().period(4))
    .node("grain", Noise::value().period(128))
    .node("albedo", Colorize::new("coarse").gradient([
        (0.0, [0.60, 0.59, 0.58]),
        (1.0, [0.74, 0.73, 0.71]),
    ]))
    .output(
        PbrOutput::new()
            .base_color("albedo")
            .roughness(Levels::new("coarse").out_range(0.85, 0.97))
            .height("grain")
            .normal_strength(0.005),
    )
    .tile_metres([2.0, 2.0])
    .build()?;
```

Every generator takes an integer period and wraps its lattice,
so the crate knows at build time whether the output meets itself at the seam.
This may seem fussy for a material,
but it is what lets a wall of repeats show no seam at any mip level,
and a graph that does not tile is an error that names the node rather than a line on a wall.

Nothing authors a normal map directly.
We bind a `height` and a `normal_strength`,
and the bake derives the normal, so relief, parallax and occlusion all read one surface.

## The default library

```rust
use ashlar_material::stdlib;

let graphs = stdlib::graphs();        // every graph, keyed `library:*`, `substances:*`, ...
let definitions = stdlib::materials(); // one MaterialDefinition per `library:*` surface
assert!(definitions.materials.contains_key("library:brick"));
```

`stdlib::materials()` answers definitions that bake at registration.
That is the right delivery for a tool and for iteration,
and the wrong one for a game at play, where a bake costs a fraction of a second in front of a frame.
The content step below turns the same definitions into files.

## The content step

`export::export` bakes every baked definition of a library and answers the files,
with no IO of its own; `ashlar-content` is the crate that writes them to disk,
beside the baked buildings.

```rust
use ashlar_material::{bake::bake_with_report, export, stdlib};

let graphs = stdlib::graphs();
let definitions = stdlib::materials();
let exported = export::export(&export::ExportRequest {
    graphs: &graphs,
    definitions: &definitions,
    directory: "materials",
    resolution: None,
    threads: std::num::NonZeroUsize::new(8),
    backend: &bake_with_report,
})?;
// `exported.library` names the files; each set's `files()` is the bytes.
```

## Where to go next

- [docs/guide/materials.md](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/materials.md) is the long guide:
  tiling, the lowering, buffered filters, shader compilation, bake costs and the standard library.
- [ADR 0003](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0003-procedural-materials.md) is the founding decision.
- The `ashlar-materials` skill under `.agents/skills/` is the authoring workflow, review loop included.

## Licence

MIT or Apache-2.0, at your option.
