# ashlar-content

The content step for [ashlar](https://github.com/JedimEmO/ashlar):
bake a game's buildings and materials into the files it ships.

A game at play links the default-feature `ashlar-bevy` and loads files.
Something has to write those files, and that is the only place the geometry
kernel and the material graph engine are needed.
This crate is that something: a library for a small binary in your game's
repository, run when content changes, never shipped.

```rust,no_run
use ashlar::LodPolicy;
use ashlar_content::Content;
use ashlar_material::stdlib;

# fn village() -> ashlar::Building { unimplemented!() }
let shipped = Content::new("../my-game/assets").ship(
    &stdlib::graphs(),
    &stdlib::materials(),
    "materials",
    &[("buildings/village.ashlar".to_owned(), village())],
    &LodPolicy::ladder(),
)?;
println!("{} material sets", shipped.materials.sets.len());
# Ok::<(), ashlar_content::ContentError>(())
```

What it writes, under the root you give it:

- `materials/<namespace>/<name>/*.ktx2` and `set.strands`: each material's maps
  with their mip chains, zstd-supercompressed, and its strand set if it grows one;
- `materials/library.materials.ron`: the library naming those files, which
  `ashlar_bevy::prelude::AshlarPlugin` loads;
- `materials/graphs.ron`: the graphs the maps came from, for tools and reviewers;
- `buildings/<name>.ashlar`: each building at every level of detail,
  with its collision proxies, portals and rooms.

`ship` resolves per-instance material overrides first, through `flatten_overrides`,
because a file cannot take a graph parameter at runtime.
Each distinct override becomes a material of its own, keyed by a hash of its parameters,
so a hundred buildings wearing the same colour cost one bake.

Every write goes through a temporary file and a rename,
so a game reading the directory while the step runs never sees half a file.
`review_pngs(true)` also writes level 0 of every map as a PNG beside its KTX2,
for looking at a material flat or comparing it with a reference;
the library still names the KTX2 files.

## Cost

This crate links `ashlar-manifold`, so it needs `cmake`, a C++ compiler and Git,
and its first build clones and compiles upstream Manifold.
That is the reason it is a separate step and not something the game does at startup.

The whole path, from dependencies to a game drawing levels of detail, is in
[the integration guide](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/integration.md),
and `examples/integration-content` in the repository is a template to copy.

## Licence

MIT or Apache-2.0, at your option.
